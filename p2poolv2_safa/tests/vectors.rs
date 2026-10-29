// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pickaxe PR #70 vectors and canonical encodings in the BCH 2026 VM.
mod test_utils;

use p2poolv2_safa::{self as safa, Leaf, SplitCommitment, TreeLeaf};
use test_utils::{vectors::*, *};

#[test]
fn leaf_preimages_and_hashes_match_pickaxe() {
    let payout = p2pkh(0x12);
    let first = Leaf {
        category: vec![0x11; 32],
        lock_hash: safa::hash256(&[0x51]),
        state_hash: vec![0xaa; 32],
        payout_hash: safa::hash256(&payout),
        target_bits: 0x207f_ffff,
        ext: vec![0; 32],
    };
    let second = Leaf {
        category: vec![0x22; 32],
        lock_hash: safa::hash256(&[0x52]),
        state_hash: vec![0xbb; 32],
        payout_hash: safa::hash256(&payout),
        target_bits: 0x1f00_ffff,
        ext: vec![0; 32],
    };
    assert_eq!(hex::encode(safa::leaf_preimage(&first).unwrap()), LEAF_A);
    let caller = FunctionCaller::new("chipnet");
    assert_eq!(
        hex::encode(
            ok(caller.call(
                14,
                &[
                    first.category.clone(),
                    first.lock_hash.clone(),
                    first.state_hash.clone(),
                    first.payout_hash.clone(),
                    uint32(first.target_bits),
                    first.ext.clone()
                ]
            ))[0]
                .clone()
        ),
        LEAF_A_HASH
    );
    assert_eq!(
        hex::encode(
            ok(caller.call(
                14,
                &[
                    second.category.clone(),
                    second.lock_hash.clone(),
                    second.state_hash.clone(),
                    second.payout_hash.clone(),
                    uint32(second.target_bits),
                    second.ext.clone()
                ]
            ))[0]
                .clone()
        ),
        LEAF_B_HASH
    );
    assert_eq!(hex::encode(safa::leaf_hash(&second).unwrap()), LEAF_B_HASH);
}
#[test]
fn two_token_tree_slots_root_and_output_match_pickaxe() {
    let first = hex::decode(LEAF_A_HASH).unwrap();
    let second = hex::decode(LEAF_B_HASH).unwrap();
    let caller = FunctionCaller::new("chipnet");
    assert_eq!(
        stack_number(&ok(caller.call(9, &[vec![0x11; 32], uint32(0), numeric(1)]))[0]),
        number(0)
    );
    assert_eq!(
        stack_number(&ok(caller.call(9, &[vec![0x22; 32], uint32(0), numeric(1)]))[0]),
        number(1)
    );
    assert_eq!(
        hex::encode(ok(caller.call(10, &[first.clone(), numeric(0), second.clone()]))[0].clone()),
        TWO_TOKEN_ROOT
    );
    assert_eq!(
        hex::encode(ok(caller.call(10, &[second.clone(), numeric(1), first.clone()]))[0].clone()),
        TWO_TOKEN_ROOT
    );
    assert_eq!(
        hex::encode(
            ok(caller.call(
                11,
                &[hex::decode(TWO_TOKEN_ROOT).unwrap(), numeric(1), uint32(0)]
            ))[0]
                .clone()
        ),
        TWO_TOKEN_OUTPUT
    );
    let tree = safa::build_tree(&[
        TreeLeaf {
            category: vec![0x11; 32],
            hash: first,
        },
        TreeLeaf {
            category: vec![0x22; 32],
            hash: second,
        },
    ])
    .unwrap();
    assert_eq!((tree.height, tree.salt, tree.slots), (1, 0, vec![0, 1]));
    assert_eq!(hex::encode(tree.root), TWO_TOKEN_ROOT);
}
#[test]
fn exact_empty_commitment() {
    assert_eq!(
        hex::encode(
            ok(FunctionCaller::new("chipnet").call(11, &[vec![0; 32], numeric(0), uint32(0)]))[0]
                .clone()
        ),
        format!(
            "00000000000000002c6a2a43544d4d02{}0000000000",
            "00".repeat(32)
        )
    );
}

#[test]
fn golden_proof_parses_rebuilds_and_verifies() {
    let proof = hex::decode(PROOF).unwrap();
    assert_eq!(proof.len(), 224);
    let caller = FunctionCaller::new("chipnet");
    let stack = ok(caller.call(1, std::slice::from_ref(&proof)));
    assert_eq!(
        (
            stack[0].len(),
            stack_number(&stack[1]),
            stack[2].len(),
            stack[3].len(),
            stack[4].len(),
            stack[5].len()
        ),
        (80, number(0), 4, 0, 59, 38)
    );
    assert_eq!(stack[6], vec![0x44; 32]);
    assert_eq!(
        stack_number(&ok(caller.call(2, &[stack[4].clone()]))[0]),
        number(325909)
    );
    let output = safa::commitment_output(&hex::decode(LEAF_A_HASH).unwrap(), 0, 0);
    assert_eq!(
        hex::encode(
            ok(caller.call(12, &[stack[4].clone(), output, stack[5].clone(), vec![]]))[0].clone()
        ),
        PROOF_COINBASE_TXID
    );
    assert_eq!(safa::decode_proof(&proof).unwrap().head, stack[4]);
    assert_eq!(
        safa::encode_proof(&safa::decode_proof(&proof).unwrap()).unwrap(),
        proof
    );
    let result = run_functions(
        "chipnet",
        &proof_body(
            &proof,
            &hex::decode(LEAF_A_HASH).unwrap(),
            &[0x11; 32],
            0x207f_ffff,
        ),
    );
    assert!(result.error.is_none(), "{:?}", result.error);
}
#[test]
fn stale_leaf_hash_fails() {
    let mut leaf = hex::decode(LEAF_A_HASH).unwrap();
    leaf[0] ^= 1;
    fails(run_functions(
        "chipnet",
        &proof_body(
            &hex::decode(PROOF).unwrap(),
            &leaf,
            &[0x11; 32],
            0x207f_ffff,
        ),
    ));
}
#[test]
fn stale_state_hash_fails() {
    let leaf = Leaf {
        category: vec![0x11; 32],
        lock_hash: safa::hash256(&[0x51]),
        state_hash: vec![0xab; 32],
        payout_hash: safa::hash256(&p2pkh(0x12)),
        target_bits: 0x207f_ffff,
        ext: vec![0; 32],
    };
    fails(run_functions(
        "chipnet",
        &proof_body(
            &hex::decode(PROOF).unwrap(),
            &safa::leaf_hash(&leaf).unwrap(),
            &[0x11; 32],
            0x207f_ffff,
        ),
    ));
}
#[test]
fn single_byte_state_hash_change_fails() {
    let mut state_hash = vec![0xaa; 32];
    state_hash[0] ^= 1;
    let leaf = Leaf {
        category: vec![0x11; 32],
        lock_hash: safa::hash256(&[0x51]),
        state_hash,
        payout_hash: safa::hash256(&p2pkh(0x12)),
        target_bits: 0x207f_ffff,
        ext: vec![0; 32],
    };
    fails(run_functions(
        "chipnet",
        &proof_body(
            &hex::decode(PROOF).unwrap(),
            &safa::leaf_hash(&leaf).unwrap(),
            &[0x11; 32],
            0x207f_ffff,
        ),
    ));
}
#[test]
fn work_above_harder_token_target_fails() {
    fails(run_functions(
        "chipnet",
        &proof_body(
            &hex::decode(PROOF).unwrap(),
            &hex::decode(LEAF_A_HASH).unwrap(),
            &[0x11; 32],
            0x0300_0001,
        ),
    ));
}
#[test]
fn golden_regtest_header_fails_mainnet_band() {
    let proof = safa::decode_proof(&hex::decode(PROOF).unwrap()).unwrap();
    fails(FunctionCaller::new("mainnet").call(
        7,
        &[
            proof.header[72..76].to_vec(),
            proof.header[68..72].to_vec(),
            numeric(325909),
        ],
    ));
}
#[test]
fn golden_regtest_header_fails_chipnet_band() {
    let proof = safa::decode_proof(&hex::decode(PROOF).unwrap()).unwrap();
    fails(FunctionCaller::new("chipnet").call(
        7,
        &[
            proof.header[72..76].to_vec(),
            proof.header[68..72].to_vec(),
            numeric(325909),
        ],
    ));
}

macro_rules! proof_edit {
    ($name:ident,$edit:expr) => {
        #[test]
        fn $name() {
            let mut proof = hex::decode(PROOF).unwrap();
            $edit(&mut proof);
            fails(FunctionCaller::new("chipnet").call(1, &[proof.clone()]));
            assert!(safa::decode_proof(&proof).is_err());
        }
    };
}
proof_edit!(proof_rejects_trailing_byte, |proof: &mut Vec<u8>| proof
    .push(0));
proof_edit!(proof_rejects_truncation, |proof: &mut Vec<u8>| {
    proof.pop();
});
proof_edit!(proof_rejects_magic, |proof: &mut Vec<u8>| proof[0] ^= 1);
proof_edit!(proof_rejects_version, |proof: &mut Vec<u8>| proof[4] ^= 1);
proof_edit!(proof_rejects_aux_height_17, |proof: &mut Vec<u8>| proof
    [85] =
    17);
#[test]
fn proof_accepts_depth_25_rejects_26() {
    let mut parts = safa::decode_proof(&hex::decode(PROOF).unwrap()).unwrap();
    parts.coinbase_branch = vec![vec![0x44; 32]; 25];
    let mut proof = safa::encode_proof(&parts).unwrap();
    ok(FunctionCaller::new("chipnet").call(1, &[proof.clone()]));
    let offset = proof.len() - 1 - 25 * 32;
    proof[offset] = 26;
    proof.extend([0; 32]);
    fails(FunctionCaller::new("chipnet").call(1, &[proof.clone()]));
    assert!(safa::decode_proof(&proof).is_err());
}
#[test]
fn whole_split_leaf_matches_pickaxe() {
    let preimage = safa::split_preimage(&SplitCommitment {
        donation_bps: 100,
        donation_hash: safa::hash256(&p2pkh(0xd0)),
        fee_bps: 100,
        fee_hash: safa::hash256(&p2pkh(0xfe)),
    })
    .unwrap();
    assert_eq!(hex::encode(&preimage), SPLIT_PREIMAGE);
    let caller = FunctionCaller::new("chipnet");
    let stack = ok(caller.call(15, &[preimage]));
    assert_eq!(hex::encode(&stack[0]), SPLIT_EXT);
    assert_eq!(stack_number(&stack[1]), number(100));
    assert_eq!(stack_number(&stack[3]), number(100));
    assert_eq!(stack[2], safa::hash256(&p2pkh(0xd0)));
    assert_eq!(stack[4], safa::hash256(&p2pkh(0xfe)));
    let leaf = Leaf {
        category: vec![0x11; 32],
        lock_hash: safa::hash256(&[0x51]),
        state_hash: vec![0xaa; 32],
        payout_hash: safa::hash256(&p2pkh(0x12)),
        target_bits: 0x207f_ffff,
        ext: stack[0].clone(),
    };
    assert_eq!(hex::encode(safa::leaf_preimage(&leaf).unwrap()), SPLIT_LEAF);
    assert_eq!(
        hex::encode(
            ok(caller.call(
                14,
                &[
                    leaf.category,
                    leaf.lock_hash,
                    leaf.state_hash,
                    leaf.payout_hash,
                    uint32(leaf.target_bits),
                    leaf.ext
                ]
            ))[0]
                .clone()
        ),
        SPLIT_LEAF_HASH
    );
    let none = ok(caller.call(15, &[vec![]]));
    assert_eq!(none[0], vec![0; 32]);
    assert_eq!(stack_number(&none[1]), number(0));
    assert_eq!(stack_number(&none[3]), number(0));
}
macro_rules! split_edit {
    ($name:ident,$accepted:expr,$edit:expr) => {
        #[test]
        fn $name() {
            let mut preimage = hex::decode(SPLIT_PREIMAGE).unwrap();
            $edit(&mut preimage);
            let result = FunctionCaller::new("chipnet").call(15, &[preimage.clone()]);
            if $accepted {
                ok(result);
                assert!(safa::parse_split(&preimage).is_ok());
            } else {
                fails(result);
                assert!(safa::parse_split(&preimage).is_err());
            }
        }
    };
}
split_edit!(split_rate_above_10000, false, |bytes: &mut Vec<u8>| bytes
    [1..3]
    .copy_from_slice(&10001u16.to_le_bytes()));
split_edit!(split_rate_10000, true, |bytes: &mut Vec<u8>| bytes[1..3]
    .copy_from_slice(&10000u16.to_le_bytes()));
split_edit!(split_nonzero_rate_zero_hash, false, |bytes: &mut Vec<
    u8,
>| bytes[3..35].fill(0));
split_edit!(split_zero_rate_nonzero_hash, false, |bytes: &mut Vec<
    u8,
>| bytes[35..37]
    .fill(0));
split_edit!(split_all_empty, false, |bytes: &mut Vec<u8>| bytes[1..]
    .fill(0));
split_edit!(split_wrong_prefix, false, |bytes: &mut Vec<u8>| bytes[0] =
    2);
split_edit!(split_short, false, |bytes: &mut Vec<u8>| bytes.truncate(68));
split_edit!(split_donation_only, true, |bytes: &mut Vec<u8>| bytes[35..]
    .fill(0));
split_edit!(split_fee_only, true, |bytes: &mut Vec<u8>| bytes[1..35]
    .fill(0));

macro_rules! share_case {
    ($name:ident,$reward:expr,$donation:expr,$fee:expr,$num:expr,$tokens:expr,$expected:expr) => {
        #[test]
        fn $name() {
            let stack = ok(FunctionCaller::new("chipnet").call(
                17,
                &[
                    big_numeric(&safa::BigInt::from_u64($reward)),
                    numeric($donation),
                    numeric($fee),
                    big_numeric(&safa::BigInt::from_u64($num)),
                    big_numeric(&safa::BigInt::from_u64($tokens)),
                ],
            ));
            let expected: [u64; 3] = $expected;
            assert_eq!(stack_number(&stack[0]).to_u64().unwrap(), expected[2]);
            assert_eq!(stack_number(&stack[1]).to_u64().unwrap(), expected[0]);
            assert_eq!(stack_number(&stack[2]).to_u64().unwrap(), expected[1]);
            let shares = safa::split_shares($reward, $donation, $fee, $num as u128, $tokens);
            assert_eq!([shares.donation, shares.fee, shares.miner], expected);
        }
    };
}
share_case!(shares_one_all, 1, 10000, 10000, 1000, 1, [1, 0, 0]);
share_case!(shares_one_9999, 1, 9999, 9999, 1000, 1, [0, 0, 1]);
share_case!(shares_99_all, 99, 10000, 10000, 1000, 1, [99, 0, 0]);
share_case!(shares_99_9999, 99, 9999, 9999, 1000, 1, [98, 0, 1]);
share_case!(shares_100_all, 100, 10000, 10000, 1000, 1, [100, 0, 0]);
share_case!(shares_100_9999, 100, 9999, 9999, 1000, 1, [99, 0, 1]);
share_case!(
    shares_max_all,
    i64::MAX as u64,
    10000,
    10000,
    1000,
    1,
    [i64::MAX as u64, 0, 0]
);
share_case!(
    shares_max_9999,
    i64::MAX as u64,
    9999,
    9999,
    1000,
    1,
    [9222449699651090329, 922244969965109, 92233720369]
);
share_case!(shares_99_small_rates, 99, 100, 200, 1000, 1, [0, 1, 98]);
share_case!(
    shares_whole_donation,
    1000000,
    10000,
    500,
    1000,
    1,
    [1000000, 0, 0]
);
share_case!(
    shares_golden,
    1000000,
    100,
    100,
    1000,
    1,
    [10000, 9900, 980100]
);
share_case!(shares_half, 1001, 5000, 10000, 1000, 1, [500, 501, 0]);
share_case!(shares_no_split, 77, 0, 0, 1000, 1, [0, 0, 77]);
share_case!(
    shares_dust_donation,
    99900,
    100,
    0,
    1000000,
    1000000,
    [0, 0, 99900]
);
share_case!(
    shares_exact_minimum,
    100000,
    100,
    0,
    1000000,
    1000000,
    [1000, 0, 99000]
);
share_case!(
    shares_drop_donation_preserves_fee,
    99900,
    100,
    200,
    1000000,
    1000000,
    [0, 1978, 97922]
);
share_case!(
    shares_no_tokens,
    1000000,
    100,
    200,
    1000000,
    0,
    [0, 0, 1000000]
);
share_case!(
    shares_golden_one_sat,
    1000000,
    100,
    100,
    1000000,
    1000000,
    [10000, 9900, 980100]
);
share_case!(
    shares_small_one_sat,
    50000,
    100,
    100,
    1000000,
    1000000,
    [0, 0, 50000]
);
#[test]
fn share_value_boundary() {
    let caller = FunctionCaller::new("chipnet");
    assert_eq!(
        stack_number(&ok(caller.call(18, &[numeric(999), numeric(1000000), numeric(1000000)]))[0]),
        number(0)
    );
    assert_eq!(
        stack_number(&ok(caller.call(18, &[numeric(1000), numeric(1000000), numeric(1000000)]))[0]),
        number(1000)
    );
}

macro_rules! token_target_case {
    ($name:ident,$bits:expr) => {
        #[test]
        fn $name() {
            let caller = FunctionCaller::new("chipnet");
            let target = stack_number(&ok(caller.call(3, &[uint32($bits)]))[0]);
            assert_eq!(target, safa::expand_token_target($bits).unwrap());
            assert_eq!(
                ok(caller.call(4, &[big_numeric(&target)]))[0],
                uint32($bits)
            );
        }
    };
}
token_target_case!(token_target_max, 0x207f_ffff);
token_target_case!(token_target_one, 0x0300_0001);
token_target_case!(token_target_pow_limit, 0x1d00_ffff);
token_target_case!(token_target_claim, 0x2001_bffe);
token_target_case!(token_target_sign_padding, 0x0400_8000);
macro_rules! invalid_token_target {
    ($name:ident,$bits:expr) => {
        #[test]
        fn $name() {
            fails(FunctionCaller::new("chipnet").call(3, &[uint32($bits)]));
            assert!(safa::expand_token_target($bits).is_err());
        }
    };
}
invalid_token_target!(token_target_exponent_two, 0x0200_0001);
invalid_token_target!(token_target_exponent_33, 0x2100_0001);
invalid_token_target!(token_target_sign_bit, 0x2080_0000);
invalid_token_target!(token_target_zero, 0x1d00_0000);
#[test]
fn token_encoding_rounds_down() {
    check_target_rounding();
}
macro_rules! header_target_case {
    ($name:ident,$bits:expr,$valid:expr) => {
        #[test]
        fn $name() {
            let result = FunctionCaller::new("chipnet").call(5, &[uint32($bits)]);
            if $valid {
                assert_eq!(
                    Some(stack_number(&ok(result)[0])),
                    safa::decode_header_bits($bits)
                );
            } else {
                fails(result);
                assert!(safa::decode_header_bits($bits).is_none());
            }
        }
    };
}
header_target_case!(header_reference, 0x1804_dafe, true);
header_target_case!(header_pow_limit, 0x1d00_ffff, true);
header_target_case!(header_chipnet, 0x1a31_06ee, true);
header_target_case!(header_bitcoin, 0x1703_0ecd, true);
header_target_case!(header_one, 0x0101_0000, true);
header_target_case!(header_ffff, 0x0300_ffff, true);
header_target_case!(header_zero, 0, false);
header_target_case!(header_negative, 0x0480_0001, false);
header_target_case!(header_overflow, 0x2300_0001, false);
header_target_case!(header_exponent_33, 0x2100_ffff, false);
header_target_case!(header_over_limit, 0x1d01_0000, false);
header_target_case!(header_regtest, 0x207f_ffff, false);
header_target_case!(header_negative_zero, 0x0480_0000, false);

macro_rules! coinbase_height_case {
    ($name:ident,$push:literal,$expected:expr) => {
        #[test]
        fn $name() {
            let coinbase = make_coinbase(
                1,
                &safa::commitment_output(&[0; 32], 0, 0),
                &[(312_500_000, p2pkh(0x12))],
                Some(&hex::decode($push).unwrap()),
            );
            let result =
                FunctionCaller::new("chipnet").call(2, std::slice::from_ref(&coinbase.head));
            if let Some(expected) = $expected {
                assert_eq!(stack_number(&ok(result)[0]), number(expected));
                assert_eq!(
                    safa::coinbase_height(&coinbase.head).unwrap(),
                    expected as u32
                );
            } else {
                fails(result);
                assert!(safa::coinbase_height(&coinbase.head).is_err());
            }
        }
    };
}
coinbase_height_case!(coinbase_height_325909, "0315f904", Some(325909));
coinbase_height_case!(coinbase_height_17, "0111", Some(17));
coinbase_height_case!(coinbase_height_128, "028000", Some(128));
coinbase_height_case!(coinbase_height_nonminimal, "021100", None::<i64>);
coinbase_height_case!(coinbase_height_negative, "0181", None::<i64>);
coinbase_height_case!(coinbase_height_opcode, "51", None::<i64>);
coinbase_height_case!(coinbase_height_empty, "00", None::<i64>);
#[test]
fn malformed_coinbase_shapes() {
    check_coinbase_shapes();
}

/// The string constant `name` in a Rust source: a literal or a `concat!` of
/// literals.
fn rust_string_constant(source: &str, name: &str) -> String {
    let start = source
        .find(&format!("const {name}: &str ="))
        .unwrap_or_else(|| panic!("no {name}"));
    let body = &source[start..start + source[start..].find(';').unwrap()];
    body.split('"').skip(1).step_by(2).collect()
}

/// The copied vectors are exactly the ones `p2poolv2_lib` checks, so both
/// sides stay held to the same bytes.
#[test]
fn vectors_match_p2pool_rust_test_sources() {
    let auxpow = include_str!("../../p2poolv2_lib/src/shares/auxpow/tests.rs");
    for (name, value) in [
        ("LEAF_A", LEAF_A),
        ("LEAF_A_HASH", LEAF_A_HASH),
        ("LEAF_B_HASH", LEAF_B_HASH),
        ("TWO_TOKEN_ROOT", TWO_TOKEN_ROOT),
        ("TWO_TOKEN_OUTPUT", TWO_TOKEN_OUTPUT),
        ("PROOF", PROOF),
        ("PROOF_COINBASE_TXID", PROOF_COINBASE_TXID),
        ("SPLIT_PREIMAGE", SPLIT_PREIMAGE),
        ("SPLIT_EXT", SPLIT_EXT),
        ("SPLIT_LEAF", SPLIT_LEAF),
        ("SPLIT_LEAF_HASH", SPLIT_LEAF_HASH),
    ] {
        assert_eq!(rust_string_constant(auxpow, name), value, "{name}");
    }
    let asert = include_str!("../../p2poolv2_lib/src/shares/bch_asert/tests.rs");
    for (height, parent, child) in test_utils::headers::MAINNET_PAIRS
        .iter()
        .chain(&test_utils::headers::CHIPNET_PAIRS)
    {
        let pair = format!("{height},\n        \"{parent}\",\n        \"{child}\",");
        assert!(
            asert.replace("\r\n", "\n").contains(&pair),
            "pair at {height}"
        );
    }
}
