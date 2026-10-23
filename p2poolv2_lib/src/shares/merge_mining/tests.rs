use super::*;
use bitcoin::consensus::deserialize;

#[test]
fn reserved_commitment_is_the_published_auxpow_layout() {
    let output = MergeMiningCommitment::reserved().output();
    let script = output.script_pubkey.as_bytes();
    assert_eq!(output.value, Amount::ZERO);
    assert_eq!(script[0], 0x6a);
    assert_eq!(script[1], AUXPOW_COMMITMENT_LENGTH as u8);
    assert_eq!(&script[2..6], &AUXPOW_MAGIC);
    assert_eq!(&script[6..38], &[0u8; 32]);
    assert_eq!(&script[38..42], &0u32.to_le_bytes());
    assert_eq!(&script[42..46], &0u32.to_le_bytes());
    assert_eq!(script.len(), 46);
}

#[test]
fn coinbase_outputs_bytes_decode_as_a_one_output_list() {
    let bytes = MergeMiningCommitment::reserved().coinbase_outputs_bytes();
    assert_eq!(bytes[0], 1);
    let outputs: Vec<TxOut> = deserialize(&bytes).unwrap();
    assert_eq!(outputs, vec![MergeMiningCommitment::reserved().output()]);
}

#[test]
fn output_parser_rejects_value_magic_and_invalid_tree() {
    let commitment = MergeMiningCommitment {
        merkle_root: [0x42; 32],
        tree_size: 4,
        nonce: 19,
    };
    assert_eq!(
        MergeMiningCommitment::from_output(&commitment.output()).unwrap(),
        commitment
    );
    let mut paid = commitment.output();
    paid.value = Amount::from_sat(1);
    assert!(MergeMiningCommitment::from_output(&paid).is_err());
    let mut malformed = commitment.output();
    let mut script = malformed.script_pubkey.into_bytes();
    script[2] ^= 1;
    malformed.script_pubkey = ScriptBuf::from_bytes(script);
    assert!(MergeMiningCommitment::from_output(&malformed).is_err());
    let invalid = MergeMiningCommitment {
        tree_size: 3,
        ..commitment
    };
    assert!(MergeMiningCommitment::from_output(&invalid.output()).is_err());
    assert!(
        MergeMiningCommitment::from_output(&TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::new()
        })
        .is_err()
    );
}
