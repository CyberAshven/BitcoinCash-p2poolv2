use super::*;
use bitcoin::consensus::{deserialize, serialize};

// Golden vectors are pinned to pickaxe-miner PR #70 at
// a1c80a1ba6ad6b1542af54f4af28058d3241c3df, docs/merge-mining.md and
// src/stratum_v2/merge/{commitment,vectors}.rs.

/// Pickaxe's golden vector for the exact empty commitment
/// (`docs/merge-mining.md`, "The empty commitment"; pickaxe-miner
/// `src/stratum_v2/merge/vectors.rs`, `the_empty_commitment`).
const EMPTY_OUTPUT: &str = concat!(
    "00000000000000002c6a2a43544d4d02",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000"
);

/// Pickaxe's two-token output 0: root `5b61ecfe...1c38d0`, `h = 1`, salt 0
/// (`vectors.rs`, `a_two_token_tree`).
const TWO_TOKEN_OUTPUT: &str = concat!(
    "00000000000000002c6a2a43544d4d025b61ecfe96c714397bc35ed813890f49",
    "f2b0287a78111e19d156f4eecd1c38d00100000000",
);

fn two_token_commitment() -> MergeMiningCommitment {
    MergeMiningCommitment {
        root: hex::decode("5b61ecfe96c714397bc35ed813890f49f2b0287a78111e19d156f4eecd1c38d0")
            .unwrap()
            .try_into()
            .unwrap(),
        height: 1,
        salt: 0,
    }
}

#[test]
fn empty_commitment_is_the_golden_53_byte_output() {
    let output = MergeMiningCommitment::empty().output();
    let bytes = serialize(&output);
    assert_eq!(bytes.len(), 53);
    assert_eq!(bytes.len(), MERGE_MINING_OUTPUT_LENGTH);
    assert_eq!(hex::encode(&bytes), EMPTY_OUTPUT);
    assert!(MergeMiningCommitment::EMPTY.is_empty());
    assert_eq!(
        MergeMiningCommitment::from_output(&deserialize(&bytes).unwrap()).unwrap(),
        MergeMiningCommitment::EMPTY
    );
}

#[test]
fn two_token_commitment_is_the_golden_output() {
    let commitment = two_token_commitment();
    let bytes = serialize(&commitment.output());
    assert_eq!(bytes.len(), 53);
    assert_eq!(hex::encode(&bytes), TWO_TOKEN_OUTPUT);
    assert!(!commitment.is_empty());
    assert_eq!(
        MergeMiningCommitment::from_output(&deserialize(&bytes).unwrap()).unwrap(),
        commitment
    );
}

/// Pickaxe's `output_matches_the_golden_bytes` (`commitment.rs`): every
/// fixed byte of a sample commitment.
#[test]
fn sample_commitment_matches_pickaxe_byte_for_byte() {
    let commitment = MergeMiningCommitment {
        root: [0x11; 32],
        height: 3,
        salt: 7,
    };
    assert_eq!(
        hex::encode(serialize(&commitment.output())),
        concat!(
            "0000000000000000",
            "2c",
            "6a",
            "2a",
            "43544d4d",
            "02",
            "1111111111111111111111111111111111111111111111111111111111111111",
            "03",
            "07000000"
        )
    );
}

#[test]
fn a_zero_root_is_valid_only_as_the_exact_empty_commitment() {
    for (height, salt) in [(1, 0), (0, 1), (16, 0xffff_ffff)] {
        let commitment = MergeMiningCommitment {
            root: [0; 32],
            height,
            salt,
        };
        assert!(commitment.validate().is_err(), "h={height} salt={salt}");
        assert!(MergeMiningCommitment::from_output(&commitment.output()).is_err());
        assert!(deserialize::<MergeMiningCommitment>(&serialize(&commitment)).is_err());
    }
    // A non-zero root is a token commitment at any height up to 16 and salt.
    for (height, salt) in [(0, 0), (16, u32::MAX)] {
        let commitment = MergeMiningCommitment {
            root: [0x01; 32],
            height,
            salt,
        };
        commitment.validate().unwrap();
        assert!(!commitment.is_empty());
    }
}

#[test]
fn tree_height_above_16_is_not_a_commitment() {
    let commitment = MergeMiningCommitment {
        root: [0x42; 32],
        height: 17,
        salt: 0,
    };
    assert!(commitment.validate().is_err());
    assert!(MergeMiningCommitment::from_output(&commitment.output()).is_err());
    assert!(deserialize::<MergeMiningCommitment>(&serialize(&commitment)).is_err());
}

#[test]
fn output_parser_rejects_value_magic_version_and_length() {
    let commitment = two_token_commitment();
    let mut paid = commitment.output();
    paid.value = Amount::from_sat(1);
    assert!(MergeMiningCommitment::from_output(&paid).is_err());
    for offset in [0, 1, 2, 6] {
        let mut script = commitment.script().into_bytes();
        script[offset] ^= 1;
        let output = TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::from_bytes(script),
        };
        assert!(
            MergeMiningCommitment::from_output(&output).is_err(),
            "offset {offset}"
        );
    }
    let mut longer = commitment.script().into_bytes();
    longer.push(0);
    assert!(
        MergeMiningCommitment::from_output(&TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::from_bytes(longer),
        })
        .is_err()
    );
    // The old fabe6d6d output is not a v2 commitment.
    let mut old = vec![0x6a, 44, 0xfa, 0xbe, 0x6d, 0x6d];
    old.extend([0; 40]);
    assert!(
        MergeMiningCommitment::from_output(&TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::from_bytes(old),
        })
        .is_err()
    );
}

#[test]
fn header_encoding_round_trips_in_37_bytes() {
    let commitment = MergeMiningCommitment {
        root: [0x33; 32],
        height: 4,
        salt: 0x0102_0304,
    };
    let bytes = serialize(&commitment);
    assert_eq!(bytes.len(), 37);
    assert_eq!(&bytes[33..], &[0x04, 0x03, 0x02, 0x01]);
    assert_eq!(
        deserialize::<MergeMiningCommitment>(&bytes).unwrap(),
        commitment
    );
}

#[test]
fn coinbase_outputs_bytes_decode_as_a_one_output_list() {
    let bytes = MergeMiningCommitment::empty().coinbase_outputs_bytes();
    assert_eq!(bytes[0], 1);
    let outputs: Vec<TxOut> = deserialize(&bytes).unwrap();
    assert_eq!(outputs, vec![MergeMiningCommitment::empty().output()]);
}
