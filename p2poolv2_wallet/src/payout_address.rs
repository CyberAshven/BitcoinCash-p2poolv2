// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Payout destination of a coinbase output: a locking script and its network.
//!
//! A share header carries its payout addresses as a network class and a
//! locking script, and this is the type that holds them. `bitcoin::Address`
//! cannot, because it has no form for Bitcoin Cash pay to script hash with a
//! 32-byte hash (P2SH32, `OP_HASH256 <32 bytes> OP_EQUAL`, from the
//! 2023-05-15 CashTokens upgrade). Every script `bitcoin::Address` has a form
//! for is held too, so the retained Bitcoin mode keeps its segwit and taproot
//! payouts.
//!
//! The network is held as its class, the part of a network a script does not
//! carry: [`Network::Bitcoin`], [`Network::Testnet`] for every test network
//! whose addresses share testnet's spelling, and [`Network::Regtest`]. Two
//! payout addresses are equal when they pay the same script on the same
//! class, whatever spelling they were parsed from.
//!
//! The text form is the `bitcoin::Address` spelling where one exists, which
//! keeps every existing log, store key and fixture unchanged. P2SH32 has no
//! such spelling and is written as CashAddr under the class's payout prefix
//! (`bitcoincash`, `bchtest` or `bchreg`). On a Bitcoin Cash node, anything
//! shown to a person goes through [`PayoutAddress::cashaddr`] instead.

use crate::bch_share::{
    BchShareNetwork, CHARSET, P2PKH_VERSION, P2SH_VERSION, P2SH32_VERSION, TOKEN_AWARE_TYPE,
    convert_bits, encode_cashaddr_payload, polymod, prefix_expand,
};
use bitcoin::hashes::Hash;
use bitcoin::opcodes::all::{OP_EQUAL, OP_HASH256};
use bitcoin::script::Builder;
use bitcoin::{Address, Network, PubkeyHash, Script, ScriptBuf, ScriptHash};
use std::fmt;
use std::str::FromStr;

/// Length of a P2SH32 locking script: `OP_HASH256`, a 32-byte push and
/// `OP_EQUAL`.
const P2SH32_SCRIPT_LENGTH: usize = 35;

/// The script a coinbase output pays, and the network class it is for.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PayoutAddress {
    network: Network,
    script: ScriptBuf,
}

/// Why a script or string is not a payout address.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PayoutAddressError {
    /// The script is not one `bitcoin::Address` has a form for, nor P2SH32.
    #[error("not a payout script: {0}")]
    Script(String),
    /// The text is neither an address nor a payout CashAddr.
    #[error("not a payout address: {0}")]
    Text(String),
}

/// The class of `network`: the network whose spelling its addresses share.
fn network_class(network: Network) -> Network {
    match network {
        Network::Bitcoin => Network::Bitcoin,
        Network::Regtest => Network::Regtest,
        _ => Network::Testnet,
    }
}

/// The class an address belongs to, from the networks it is valid for.
/// A base58 address cannot tell testnet from regtest, so it is testnet's.
fn address_class(address: &Address) -> Network {
    let unchecked = address.as_unchecked();
    if unchecked.is_valid_for_network(Network::Bitcoin) {
        Network::Bitcoin
    } else if unchecked.is_valid_for_network(Network::Testnet) {
        Network::Testnet
    } else {
        Network::Regtest
    }
}

/// The Bitcoin Cash payout prefix of a network class.
fn payout_prefix(network: Network) -> &'static str {
    match network_class(network) {
        Network::Bitcoin => "bitcoincash",
        Network::Regtest => "bchreg",
        _ => "bchtest",
    }
}

/// The network class a Bitcoin Cash payout prefix names.
fn network_for_payout_prefix(prefix: &str) -> Option<Network> {
    match prefix {
        "bitcoincash" => Some(Network::Bitcoin),
        "bchtest" => Some(Network::Testnet),
        "bchreg" => Some(Network::Regtest),
        _ => None,
    }
}

/// True for `OP_HASH256 <32 bytes> OP_EQUAL`.
fn is_p2sh32(script: &Script) -> bool {
    let bytes = script.as_bytes();
    bytes.len() == P2SH32_SCRIPT_LENGTH
        && bytes[0] == OP_HASH256.to_u8()
        && bytes[1] == 32
        && bytes[34] == OP_EQUAL.to_u8()
}

impl PayoutAddress {
    /// The payout `address` pays.
    pub fn from_address(address: &Address) -> Self {
        Self {
            network: address_class(address),
            script: address.script_pubkey(),
        }
    }

    /// Pay to the 32-byte script hash `hash` on `network`.
    pub fn p2sh32(hash: [u8; 32], network: Network) -> Self {
        Self {
            network: network_class(network),
            script: Builder::new()
                .push_opcode(OP_HASH256)
                .push_slice(hash)
                .push_opcode(OP_EQUAL)
                .into_script(),
        }
    }

    /// The payout that locks with `script` on `network`.
    ///
    /// Accepts every script `bitcoin::Address` has a form for, and P2SH32.
    /// Whether a chain allows a given script is the chain's rule, checked
    /// where its shares are validated.
    pub fn from_script(script: &Script, network: Network) -> Result<Self, PayoutAddressError> {
        if is_p2sh32(script) {
            return Ok(Self {
                network: network_class(network),
                script: script.to_owned(),
            });
        }
        let address = Address::from_script(script, network)
            .map_err(|error| PayoutAddressError::Script(error.to_string()))?;
        Ok(Self::from_address(&address))
    }

    /// The locking script this payout pays.
    pub fn script_pubkey(&self) -> ScriptBuf {
        self.script.clone()
    }

    /// The locking script this payout pays, borrowed.
    pub fn as_script(&self) -> &Script {
        &self.script
    }

    /// The network class: Bitcoin, Testnet (every test network) or Regtest.
    pub fn network(&self) -> Network {
        self.network
    }

    /// The `bitcoin::Address` for this payout, or `None` for P2SH32, which
    /// has no such form.
    pub fn to_address(&self) -> Option<Address> {
        Address::from_script(&self.script, self.network).ok()
    }

    /// True when this pays a 32-byte script hash (P2SH32).
    pub fn is_p2sh32(&self) -> bool {
        is_p2sh32(&self.script)
    }

    /// The 32-byte script hash a P2SH32 payout pays.
    pub fn p2sh32_hash(&self) -> Option<[u8; 32]> {
        if !self.is_p2sh32() {
            return None;
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&self.script.as_bytes()[2..34]);
        Some(hash)
    }

    /// The CashAddr of this payout under `network`'s payout prefix, for
    /// P2PKH, P2SH20 and P2SH32. Any other script has no CashAddr.
    pub fn cashaddr(&self, network: BchShareNetwork) -> Option<String> {
        self.cashaddr_under(network.payout_prefix())
    }

    fn cashaddr_under(&self, prefix: &str) -> Option<String> {
        let script = self.as_script();
        let bytes = script.as_bytes();
        let (version, hash) = if script.is_p2pkh() {
            (P2PKH_VERSION, &bytes[3..23])
        } else if script.is_p2sh() {
            (P2SH_VERSION, &bytes[2..22])
        } else if is_p2sh32(script) {
            (P2SH32_VERSION, &bytes[2..34])
        } else {
            return None;
        };
        Some(encode_cashaddr_payload(prefix, version, hash))
    }

    /// Decode a payout CashAddr: P2PKH, P2SH20 or P2SH32, token-aware or not.
    /// The prefix sets the network class.
    pub fn from_cashaddr(text: &str) -> Result<Self, PayoutAddressError> {
        let invalid = |reason: &str| PayoutAddressError::Text(format!("{text}: {reason}"));
        let lowered = if text.bytes().any(|byte| byte.is_ascii_uppercase()) {
            if text.bytes().any(|byte| byte.is_ascii_lowercase()) {
                return Err(invalid("mixed case"));
            }
            text.to_ascii_lowercase()
        } else {
            text.to_string()
        };
        let (prefix, payload) = lowered
            .split_once(':')
            .ok_or_else(|| invalid("no CashAddr prefix"))?;
        let network =
            network_for_payout_prefix(prefix).ok_or_else(|| invalid("not a payout prefix"))?;
        let mut symbols = Vec::with_capacity(payload.len());
        for character in payload.bytes() {
            let index = CHARSET
                .iter()
                .position(|symbol| *symbol == character)
                .ok_or_else(|| invalid("character outside the CashAddr alphabet"))?;
            symbols.push(index as u8);
        }
        if symbols.len() <= 8 {
            return Err(invalid("too short"));
        }
        let mut checked = prefix_expand(prefix);
        checked.extend_from_slice(&symbols);
        if polymod(&checked) != 1 {
            return Err(invalid("bad checksum"));
        }
        let bytes = convert_bits(&symbols[..symbols.len() - 8], 5, 8, false)
            .ok_or_else(|| invalid("bad padding"))?;
        let (version, hash) = bytes.split_first().ok_or_else(|| invalid("empty"))?;
        match (version & !TOKEN_AWARE_TYPE, hash.len()) {
            (P2PKH_VERSION, 20) => Ok(Self::from_address(&Address::p2pkh(
                PubkeyHash::from_slice(hash).expect("20 bytes"),
                network,
            ))),
            (P2SH_VERSION, 20) => Ok(Self::from_address(&Address::p2sh_from_hash(
                ScriptHash::from_slice(hash).expect("20 bytes"),
                network,
            ))),
            (P2SH32_VERSION, 32) => {
                let mut hash32 = [0u8; 32];
                hash32.copy_from_slice(hash);
                Ok(Self::p2sh32(hash32, network))
            }
            _ => Err(invalid("not a P2PKH, P2SH20 or P2SH32 payout")),
        }
    }
}

impl From<Address> for PayoutAddress {
    fn from(address: Address) -> Self {
        Self::from_address(&address)
    }
}

impl From<&Address> for PayoutAddress {
    fn from(address: &Address) -> Self {
        Self::from_address(address)
    }
}

impl From<&PayoutAddress> for PayoutAddress {
    fn from(address: &PayoutAddress) -> Self {
        address.clone()
    }
}

impl fmt::Display for PayoutAddress {
    /// The `bitcoin::Address` spelling, or CashAddr for P2SH32.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(address) = self.to_address() {
            return write!(formatter, "{address}");
        }
        match self.cashaddr_under(payout_prefix(self.network)) {
            Some(cashaddr) => write!(formatter, "{cashaddr}"),
            None => write!(formatter, "script:{}", self.script.to_hex_string()),
        }
    }
}

impl fmt::Debug for PayoutAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "PayoutAddress({self})")
    }
}

impl FromStr for PayoutAddress {
    type Err = PayoutAddressError;

    /// Parse the text form: a `bitcoin::Address`, or a payout CashAddr.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if let Ok(address) = text.parse::<Address<_>>() {
            return Ok(Self::from_address(&address.assume_checked()));
        }
        Self::from_cashaddr(text)
    }
}

impl serde::Serialize for PayoutAddress {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for PayoutAddress {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 32-byte P2SH vector of the CashAddr specification.
    const SPEC_P2SH32_HASH: [u8; 32] = [
        0x31, 0x73, 0xef, 0x66, 0x23, 0xc6, 0xb4, 0x8f, 0xfd, 0x1a, 0x3d, 0xcc, 0x0c, 0xc6, 0x48,
        0x9b, 0x0a, 0x07, 0xbb, 0x47, 0xa3, 0x7f, 0x47, 0xcf, 0xef, 0x4f, 0xe6, 0x9d, 0xe8, 0x25,
        0xc0, 0x60,
    ];
    const SPEC_P2SH32_CASHADDR: &str =
        "bchtest:pvch8mmxy0rtfrlarg7ucrxxfzds5pamg73h7370aa87d80gyhqxq7fqng6m6";

    #[test]
    fn a_p2sh32_payout_pays_hash256_of_its_hash_and_equal() {
        let payout = PayoutAddress::p2sh32(SPEC_P2SH32_HASH, Network::Testnet);
        let mut expected = vec![0xaa, 0x20];
        expected.extend_from_slice(&SPEC_P2SH32_HASH);
        expected.push(0x87);
        assert_eq!(payout.script_pubkey().as_bytes(), expected);
        assert!(payout.is_p2sh32());
        assert_eq!(payout.p2sh32_hash(), Some(SPEC_P2SH32_HASH));
        assert_eq!(payout.to_address(), None);
    }

    #[test]
    fn a_p2sh32_payout_is_written_and_read_as_the_specification_cashaddr() {
        let payout = PayoutAddress::p2sh32(SPEC_P2SH32_HASH, Network::Testnet);
        assert_eq!(payout.to_string(), SPEC_P2SH32_CASHADDR);
        assert_eq!(
            payout.cashaddr(BchShareNetwork::Chipnet).as_deref(),
            Some(SPEC_P2SH32_CASHADDR)
        );
        assert_eq!(
            SPEC_P2SH32_CASHADDR.parse::<PayoutAddress>().unwrap(),
            payout
        );
        assert_eq!(
            SPEC_P2SH32_CASHADDR
                .to_ascii_uppercase()
                .parse::<PayoutAddress>()
                .unwrap(),
            payout
        );
    }

    /// A token-aware P2SH32 (type 3, `r`) pays the same P2SH32 script, and
    /// is written back in its plain `p` form.
    #[test]
    fn a_token_aware_p2sh32_cashaddr_pays_p2sh32() {
        let token_aware = PayoutAddress::from_cashaddr(
            "bitcoincash:rvqqqqqqqqqqqqqqqqqqqqqqzg69v7ysqqqqqqqqqqqqqqqqqqqqqn9alsp2y",
        )
        .unwrap();
        assert!(token_aware.is_p2sh32());
        assert_eq!(token_aware.network(), Network::Bitcoin);
        let plain = token_aware.to_string();
        assert!(plain.starts_with("bitcoincash:p"));
        assert_eq!(plain.parse::<PayoutAddress>().unwrap(), token_aware);
    }

    #[test]
    fn a_legacy_payout_keeps_its_bitcoin_spelling_and_round_trips() {
        let address = "1BpEi6DfDAUFd7GtittLSdBeYJvcoaVggu"
            .parse::<Address<_>>()
            .unwrap()
            .assume_checked();
        let payout = PayoutAddress::from_address(&address);
        assert_eq!(payout.to_string(), "1BpEi6DfDAUFd7GtittLSdBeYJvcoaVggu");
        assert_eq!(payout.to_address(), Some(address));
        assert_eq!(
            payout.cashaddr(BchShareNetwork::Mainnet).as_deref(),
            Some("bitcoincash:qpm2qsznhks23z7629mms6s4cwef74vcwvy22gdx6a")
        );
        assert_eq!(
            "bitcoincash:qpm2qsznhks23z7629mms6s4cwef74vcwvy22gdx6a"
                .parse::<PayoutAddress>()
                .unwrap(),
            payout
        );
    }

    #[test]
    fn from_script_accepts_p2sh32_and_address_scripts_and_refuses_others() {
        let p2sh32 = PayoutAddress::p2sh32(SPEC_P2SH32_HASH, Network::Regtest);
        assert_eq!(
            PayoutAddress::from_script(p2sh32.as_script(), Network::Regtest).unwrap(),
            p2sh32
        );
        let p2pkh = ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([7; 20]));
        assert_eq!(
            PayoutAddress::from_script(&p2pkh, Network::Bitcoin)
                .unwrap()
                .script_pubkey(),
            p2pkh
        );
        let bare = Builder::new().push_opcode(OP_EQUAL).into_script();
        assert!(PayoutAddress::from_script(&bare, Network::Bitcoin).is_err());
    }

    #[test]
    fn a_regtest_p2sh32_keeps_the_regtest_prefix() {
        let payout = PayoutAddress::p2sh32([0x11; 32], Network::Regtest);
        assert!(payout.to_string().starts_with("bchreg:p"));
        assert_eq!(payout.to_string().parse::<PayoutAddress>().unwrap(), payout);
    }

    #[test]
    fn serde_round_trips_through_the_text_form() {
        let payout = PayoutAddress::p2sh32(SPEC_P2SH32_HASH, Network::Testnet);
        let json = serde_json::to_string(&payout).unwrap();
        assert_eq!(json, format!("\"{SPEC_P2SH32_CASHADDR}\""));
        assert_eq!(
            serde_json::from_str::<PayoutAddress>(&json).unwrap(),
            payout
        );
    }
}
