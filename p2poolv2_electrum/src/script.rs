// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `scriptPubKey` rendering for verbose transactions, in the shape the
//! Electrum protocol docs show (the pre-v22 bitcoind shape): `asm`, `hex`,
//! `type`, and `reqSigs` and `addresses` when they apply.

use bitcoin::{Network, Script, WitnessProgram, WitnessVersion};
use p2poolv2_lib::address::Address;
use p2poolv2_lib::address::bch_share::{BchShareAddress, BchShareNetwork};
use p2poolv2_lib::address::share_owner::CASH_P2PKH_HASH_LENGTH;
use serde_json::{Map, Value};

/// A P2TR script is `OP_1 OP_PUSHBYTES_32 <32-byte output key>`.
const P2TR_PROGRAM_OFFSET: usize = 2;

/// A P2PKH script is `OP_DUP OP_HASH160 OP_PUSHBYTES_20 <20-byte hash>
/// OP_EQUALVERIFY OP_CHECKSIG`.
const P2PKH_HASH_OFFSET: usize = 3;

/// Render `script` as a bitcoind `scriptPubKey` object.
///
/// `asm` is rust-bitcoin's format (`OP_PUSHBYTES_32 <hex>`), not bitcoind's;
/// wallets read `hex`. `addresses` holds the share chain (p2pool) address
/// for a P2TR output, the only output type a share chain address encodes,
/// so no bitcoin address is ever shown for a share chain output. `reqSigs`
/// is 1 for single-key output types and absent otherwise.
///
/// On a Bitcoin Cash node (`cash_network` is `Some`) a share chain address
/// encodes a P2PKH output instead, as a CashAddr share address (`tp2pool:`
/// on chipnet); see `cash_share_address`.
pub(crate) fn script_pubkey_json(
    script: &Script,
    network: Network,
    cash_network: Option<BchShareNetwork>,
) -> Value {
    let script_type = script_type(script);
    let mut fields = Map::with_capacity(5);
    fields.insert("asm".to_string(), Value::from(script.to_asm_string()));
    fields.insert("hex".to_string(), Value::from(script.to_hex_string()));
    if is_single_key(script_type) {
        fields.insert("reqSigs".to_string(), Value::from(1));
    }
    fields.insert("type".to_string(), Value::from(script_type));
    let address = match cash_network {
        Some(cash_network) => cash_share_address(script, cash_network),
        None => share_chain_address(script, network).map(|address| address.to_string()),
    };
    if let Some(address) = address {
        fields.insert(
            "addresses".to_string(),
            Value::Array(vec![Value::from(address)]),
        );
    }
    Value::Object(fields)
}

/// bitcoind's output type name for `script`.
fn script_type(script: &Script) -> &'static str {
    if script.is_p2tr() {
        "witness_v1_taproot"
    } else if script.is_p2wpkh() {
        "witness_v0_keyhash"
    } else if script.is_p2wsh() {
        "witness_v0_scripthash"
    } else if script.is_witness_program() {
        "witness_unknown"
    } else if script.is_p2pkh() {
        "pubkeyhash"
    } else if script.is_p2sh() {
        "scripthash"
    } else if script.is_p2pk() {
        "pubkey"
    } else if script.is_multisig() {
        "multisig"
    } else if script.is_op_return() {
        "nulldata"
    } else {
        "nonstandard"
    }
}

fn is_single_key(script_type: &str) -> bool {
    matches!(
        script_type,
        "witness_v1_taproot" | "witness_v0_keyhash" | "pubkeyhash" | "pubkey"
    )
}

/// The share chain address a P2TR output pays, or `None` for any other
/// script: a share chain address only encodes a 32-byte witness v1 program.
fn share_chain_address(script: &Script, network: Network) -> Option<Address> {
    if !script.is_p2tr() {
        return None;
    }
    let output_key = &script.as_bytes()[P2TR_PROGRAM_OFFSET..];
    let witness_program = WitnessProgram::new(WitnessVersion::V1, output_key).ok()?;
    Address::from_witness_program(witness_program, network).ok()
}

/// The Bitcoin Cash share address a P2PKH output pays, or `None` for any
/// other script: a Bitcoin Cash share address only encodes a P2PKH key hash.
fn cash_share_address(script: &Script, cash_network: BchShareNetwork) -> Option<String> {
    if !script.is_p2pkh() {
        return None;
    }
    let hash = script
        .as_bytes()
        .get(P2PKH_HASH_OFFSET..P2PKH_HASH_OFFSET + CASH_P2PKH_HASH_LENGTH)?
        .try_into()
        .ok()?;
    Some(BchShareAddress::new(cash_network, hash).encode())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;
    use bitcoin::script::PushBytesBuf;
    use bitcoin::{ScriptBuf, ScriptHash, WPubkeyHash};
    use p2poolv2_lib::address::share_owner::{self, ShareOwner};
    use p2poolv2_lib::test_utils::make_test_share_address;
    use serde_json::json;

    #[test]
    fn share_output_renders_taproot_with_p2pool_address() {
        let address = make_test_share_address(1, Network::Signet);
        let script = address.script_pubkey();

        let rendered = script_pubkey_json(&script, Network::Signet, None);

        assert_eq!(
            rendered,
            json!({
                "asm": script.to_asm_string(),
                "hex": script.to_hex_string(),
                "reqSigs": 1,
                "type": "witness_v1_taproot",
                "addresses": [address.to_string()],
            })
        );
        assert!(
            rendered["addresses"][0]
                .as_str()
                .unwrap()
                .starts_with("sp2pool1")
        );
        assert!(
            rendered["asm"]
                .as_str()
                .unwrap()
                .starts_with("OP_PUSHNUM_1 OP_PUSHBYTES_32 ")
        );
    }

    #[test]
    fn op_return_renders_nulldata_without_addresses_or_req_sigs() {
        let data = PushBytesBuf::try_from(b"p2pool".to_vec()).unwrap();
        let script = ScriptBuf::new_op_return(data);

        let rendered = script_pubkey_json(&script, Network::Signet, None);

        assert_eq!(rendered["type"], json!("nulldata"));
        assert!(rendered.get("addresses").is_none());
        assert!(rendered.get("reqSigs").is_none());
    }

    #[test]
    fn p2wpkh_renders_keyhash_with_req_sigs_and_no_addresses() {
        let script = ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([7; 20]));

        let rendered = script_pubkey_json(&script, Network::Signet, None);

        assert_eq!(rendered["type"], json!("witness_v0_keyhash"));
        assert_eq!(rendered["reqSigs"], json!(1));
        assert!(rendered.get("addresses").is_none());
    }

    /// A witness v1 program that is not 32 bytes (e.g. a P2A anchor) is not
    /// taproot, and a share chain address cannot encode it.
    #[test]
    fn short_witness_v1_program_renders_unknown_without_addresses() {
        let program = WitnessProgram::new(WitnessVersion::V1, &[0x4e, 0x73]).unwrap();
        let script = ScriptBuf::new_witness_program(&program);

        let rendered = script_pubkey_json(&script, Network::Signet, None);

        assert_eq!(rendered["type"], json!("witness_unknown"));
        assert!(rendered.get("addresses").is_none());
        assert!(rendered.get("reqSigs").is_none());
    }

    #[test]
    fn bare_op_true_renders_nonstandard() {
        let script = ScriptBuf::from_bytes(vec![0x51]);

        let rendered = script_pubkey_json(&script, Network::Signet, None);

        assert_eq!(rendered["type"], json!("nonstandard"));
        assert!(rendered.get("reqSigs").is_none());
        assert!(rendered.get("addresses").is_none());
    }

    /// A Bitcoin Cash share coinbase pays P2PKH. The text is the CashAddr
    /// share address of that owner (`tp2pool:` on chipnet), not a bitcoin
    /// address.
    #[test]
    fn p2pkh_on_bitcoin_cash_renders_the_share_address() {
        let owner = ShareOwner::CashP2pkh([0x22; 20]);
        let script = owner.script_pubkey();
        let cash_network = BchShareNetwork::Chipnet;
        let address = share_owner::to_address_string(&owner, Network::Testnet, Some(cash_network));

        let rendered = script_pubkey_json(&script, Network::Testnet, Some(cash_network));

        assert_eq!(rendered["type"], json!("pubkeyhash"));
        assert_eq!(rendered["reqSigs"], json!(1));
        assert_eq!(rendered["addresses"], json!([address]));
        assert!(address.starts_with("tp2pool:"));
    }

    /// Bitcoin still shows a share address only for P2TR. A P2PKH output
    /// has no share-chain spelling there.
    #[test]
    fn p2pkh_on_bitcoin_shows_no_addresses() {
        let script = ShareOwner::CashP2pkh([0x22; 20]).script_pubkey();

        let rendered = script_pubkey_json(&script, Network::Signet, None);

        assert_eq!(rendered["type"], json!("pubkeyhash"));
        assert_eq!(rendered["reqSigs"], json!(1));
        assert!(rendered.get("addresses").is_none());
    }

    /// A bech32 share address is a Bitcoin address. A Bitcoin Cash node
    /// does not show one for a P2TR output.
    #[test]
    fn taproot_on_bitcoin_cash_shows_no_addresses() {
        let address = make_test_share_address(1, Network::Signet);
        let script = address.script_pubkey();

        let rendered =
            script_pubkey_json(&script, Network::Testnet, Some(BchShareNetwork::Chipnet));

        assert_eq!(rendered["type"], json!("witness_v1_taproot"));
        assert!(rendered.get("addresses").is_none());
    }

    #[test]
    fn p2sh_on_bitcoin_cash_shows_no_addresses() {
        let script = ScriptBuf::new_p2sh(&ScriptHash::from_byte_array([9; 20]));

        let rendered =
            script_pubkey_json(&script, Network::Testnet, Some(BchShareNetwork::Chipnet));

        assert_eq!(rendered["type"], json!("scripthash"));
        assert!(rendered.get("addresses").is_none());
    }

    #[test]
    fn op_return_on_bitcoin_cash_shows_no_addresses() {
        let data = PushBytesBuf::try_from(b"p2pool".to_vec()).unwrap();
        let script = ScriptBuf::new_op_return(data);

        let rendered =
            script_pubkey_json(&script, Network::Testnet, Some(BchShareNetwork::Chipnet));

        assert_eq!(rendered["type"], json!("nulldata"));
        assert!(rendered.get("addresses").is_none());
        assert!(rendered.get("reqSigs").is_none());
    }
}
