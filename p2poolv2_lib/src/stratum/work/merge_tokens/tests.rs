// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::OutputPair;
use crate::shares::auxpow::SplitSlot;
use crate::stratum::work::coinbase::build_coinbase;
use crate::test_utils::make_test_address;
use bitcoin::block::Version as BlockVersion;
use bitcoin::hashes::sha256;
use bitcoin::script::PushBytesBuf;
use bitcoin::transaction::Version;
use bitcoin::{Amount, BlockHash, CompactTarget};
use std::io::Write;

/// P2PKH to twenty `byte` bytes.
fn p2pkh(byte: u8) -> Vec<u8> {
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend([byte; 20]);
    script.extend([0x88, 0xac]);
    script
}

/// Pickaxe's token A (PR #70, `docs/merge-mining.md`): category 11 x 32,
/// baton locking bytecode `51`, state aa x 32, target `0x207fffff`.
fn token_a() -> MergeMinedToken {
    MergeMinedToken {
        category: [0x11; 32],
        lock_hash: bitcoin::hashes::sha256d::Hash::hash(&[0x51]).to_byte_array(),
        state_hash: [0xaa; 32],
        target_bits: 0x207f_ffff,
        max_height: MAX_MERGE_MINING_TREE_HEIGHT,
        safa: None,
    }
}

fn token_b() -> MergeMinedToken {
    MergeMinedToken {
        category: [0x22; 32],
        lock_hash: bitcoin::hashes::sha256d::Hash::hash(&[0x52]).to_byte_array(),
        state_hash: [0xbb; 32],
        target_bits: 0x1f00_ffff,
        max_height: MAX_MERGE_MINING_TREE_HEIGHT,
        safa: None,
    }
}

/// A 1% donation to P2PKH(d0 x 20) and a 1% fee to P2PKH(fe x 20).
fn pickaxe_split() -> TokenSplit {
    TokenSplit {
        donation: SplitSlot::new(100, &p2pkh(0xd0)).unwrap(),
        fee: SplitSlot::new(100, &p2pkh(0xfe)).unwrap(),
    }
}

/// A node mining Pickaxe's token A for a miner paid to P2PKH(12 x 20), with
/// a 1% P2Pool donation and a 1% fee, builds Pickaxe's whole split leaf
/// byte for byte (leaf hash `c4027136...033e41`), and alone in its tree
/// that leaf hash is the miner's root.
#[test]
fn a_miner_s_leaf_matches_pickaxe_s_split_leaf() {
    let set = TokenSet::new(vec![token_a()]).unwrap();
    let job = set.miner_job(&p2pkh(0x12), pickaxe_split());
    assert_eq!(
        hex::encode(job.entries[0].leaf.hash()),
        "c40271368b4287df8fe3430e1bc3f9410b554874de698144fa04fe5df7033e41"
    );
    assert_eq!(
        hex::encode(job.entries[0].leaf.ext),
        "79f31f395015b16c76e12673ec8f58329d81f1884d76a0d210d1f3158a8faa85"
    );
    assert_eq!(job.commitment.root, job.entries[0].leaf.hash());
    assert_eq!((job.commitment.height, job.commitment.salt), (0, 0));
}

/// Two miners share the set's layout and differ in their root: each leaf
/// binds its miner's payout, and each miner's disclosure rebuilds their own
/// root and not the other's. The tree is the one `AuxiliaryTree::build`
/// makes of the same leaves.
#[test]
fn each_miner_mines_a_root_of_their_own() {
    let set = TokenSet::new(vec![token_a(), token_b()]).unwrap();
    let first = set.miner_job(&p2pkh(0x12), pickaxe_split());
    let second = set.miner_job(&p2pkh(0x34), pickaxe_split());
    assert_eq!(
        (first.commitment.height, first.commitment.salt),
        (second.commitment.height, second.commitment.salt)
    );
    assert_ne!(first.commitment.root, second.commitment.root);
    assert_eq!(
        first.entries[1].leaf.payout_hash,
        bitcoin::hashes::sha256d::Hash::hash(&p2pkh(0x12)).to_byte_array()
    );
    let leaves: Vec<TokenLeaf> = first.entries.iter().map(|entry| entry.leaf).collect();
    assert_eq!(
        AuxiliaryTree::build(&leaves).unwrap().commitment,
        first.commitment
    );
    assert_eq!(first.disclosure().check_root(&first.commitment), Ok(()));
    assert!(first.disclosure().check_root(&second.commitment).is_err());
    assert_eq!(first.branches().len(), 2);
    assert_eq!(
        first.output().to_vec(),
        serialize(&first.commitment.output())
    );
}

/// A miner who chose no donation, on a node with no fee, mines leaves with
/// a zero `ext`, and their disclosure carries no split.
#[test]
fn a_miner_without_cuts_mines_a_zero_ext() {
    let set = TokenSet::new(vec![token_a()]).unwrap();
    let job = set.miner_job(&p2pkh(0x12), TokenSplit::default());
    assert_eq!(job.entries[0].leaf.ext, [0; 32]);
    let disclosure = job.disclosure().encode();
    assert_eq!(disclosure.len(), 6 + 169 + 1);
}

/// The token file: 32-byte hex values, eight hex digits of target, an
/// optional tree limit; an empty list merge-mines nothing.
#[test]
fn a_token_file_reads_its_tokens() {
    let json = format!(
        r#"{{"tokens":[{{"category":"{}","lock_hash":"{}","state_hash":"{}","target_bits":"207fffff"}},{{"category":"{}","lock_hash":"{}","state_hash":"{}","target_bits":"1f00ffff","max_height":4}}]}}"#,
        hex::encode([0x11; 32]),
        hex::encode(token_a().lock_hash),
        hex::encode([0xaa; 32]),
        hex::encode([0x22; 32]),
        hex::encode(token_b().lock_hash),
        hex::encode([0xbb; 32]),
    );
    let set = TokenSet::from_json(json.as_bytes()).unwrap().unwrap();
    assert_eq!(set.tokens()[0], token_a());
    assert_eq!(
        set.tokens()[1],
        MergeMinedToken {
            max_height: 4,
            ..token_b()
        }
    );
    assert_eq!(set.layout().height, 1);
    assert_eq!(TokenSet::from_json(br#"{"tokens":[]}"#), Ok(None));
}

/// The JSON of one token with `category`, `target_bits` and `extra` fields.
fn one_token_json(category: &str, target_bits: &str, extra: &str) -> String {
    format!(
        r#"{{"tokens":[{{"category":"{category}","lock_hash":"{}","state_hash":"{}","target_bits":"{target_bits}"{extra}}}]}}"#,
        hex::encode([0x33; 32]),
        hex::encode([0x44; 32]),
    )
}

/// A token file that could not be mined as written is refused, saying why:
/// a category that is not 32 bytes, a target that is not eight digits or
/// not canonical, an unknown field, a tree limit above 16, or no JSON.
#[test]
fn a_token_file_that_cannot_be_mined_is_refused() {
    let category = hex::encode([0x11; 32]);
    assert!(TokenSet::from_json(one_token_json(&category, "207fffff", "").as_bytes()).is_ok());
    assert!(TokenSet::from_json(one_token_json("1111", "207fffff", "").as_bytes()).is_err());
    assert!(TokenSet::from_json(one_token_json(&category, "7fffff", "").as_bytes()).is_err());
    assert!(TokenSet::from_json(one_token_json(&category, "20800000", "").as_bytes()).is_err());
    assert!(
        TokenSet::from_json(one_token_json(&category, "207fffff", r#","ext":"00""#).as_bytes())
            .is_err()
    );
    assert!(
        TokenSet::from_json(
            one_token_json(&category, "207fffff", r#","max_height":17"#).as_bytes()
        )
        .is_err()
    );
    assert!(TokenSet::from_json(b"not json").is_err());
}

/// Two tokens need a tree at least 1 high; one whose covenant accepts no
/// taller tree than 0 cannot share it, and the set is refused. The same
/// category twice, or no token, is refused too.
#[test]
fn a_token_set_must_fit_its_lowest_tree_limit() {
    let low = MergeMinedToken {
        max_height: 0,
        ..token_b()
    };
    assert!(
        TokenSet::new(vec![token_a(), low])
            .unwrap_err()
            .contains("at most 0 high")
    );
    assert!(TokenSet::new(vec![token_a(), token_a()]).is_err());
    assert!(TokenSet::new(Vec::new()).is_err());
}

/// The coinbase of a pool-built job carrying `job`'s root, as the stratum
/// builder lays it out.
fn job_coinbase(job: &MinerTokenJob) -> Transaction {
    build_coinbase(
        Version::TWO,
        &[OutputPair {
            address: make_test_address(1),
            amount: Amount::from_sat(312_500_000),
        }],
        702,
        PushBytesBuf::from(&[0u8]),
        None,
        b"P2Poolv2",
        Some(sha256::Hash::from_byte_array([0x77; 32])),
        0,
        Some(&[0u8; 12]),
        Some(job.commitment),
        None,
    )
    .unwrap()
}

/// A header on the miner's coinbase.
fn header_on(coinbase: &Transaction, nonce: u32) -> Header {
    Header {
        version: BlockVersion::from_consensus(0x2000_0000),
        prev_blockhash: BlockHash::all_zeros(),
        merkle_root: TxMerkleNode::from_raw_hash(coinbase.compute_txid().to_raw_hash()),
        time: 1_700_000_000,
        bits: CompactTarget::from_consensus(0x207f_ffff),
        nonce,
    }
}

/// A share whose hash meets token A's target wins it: the proof the node
/// gives verifies against the miner's leaf as a covenant would, and carries
/// the split the claim reveals. Token B's harder target is not met by the
/// same share.
#[test]
fn a_share_meeting_a_token_target_wins_a_proof_that_verifies() {
    let set = TokenSet::new(vec![token_a(), token_b()]).unwrap();
    let job = set.miner_job(&p2pkh(0x12), pickaxe_split());
    let coinbase = job_coinbase(&job);
    let winning = (0u32..)
        .map(|nonce| header_on(&coinbase, nonce))
        .find(|header| job.entries[0].is_won_by(&header.block_hash()))
        .unwrap();
    assert!(!job.entries[1].is_won_by(&winning.block_hash()));

    let wins = job.wins(&winning, &coinbase, &[], None);

    assert_eq!(wins.len(), 1);
    assert_eq!(wins[0].category, [0x11; 32]);
    assert_eq!(wins[0].split, pickaxe_split());
    let proof = AuxProof::from_bytes(&wins[0].proof).unwrap();
    assert_eq!(
        proof.verify(&job.entries[0].leaf, MAX_MERGE_MINING_TREE_HEIGHT, None),
        Ok(())
    );
    assert!(
        proof
            .verify(&job.entries[1].leaf, MAX_MERGE_MINING_TREE_HEIGHT, None)
            .is_err()
    );
}

/// A share that meets no token's target wins nothing.
#[test]
fn a_share_below_every_token_target_wins_nothing() {
    let hard = MergeMinedToken {
        target_bits: 0x0300_0001,
        ..token_a()
    };
    let set = TokenSet::new(vec![hard]).unwrap();
    let job = set.miner_job(&p2pkh(0x12), pickaxe_split());
    let coinbase = job_coinbase(&job);
    assert!(
        job.wins(&header_on(&coinbase, 0), &coinbase, &[], None)
            .is_empty()
    );
}

/// Write a token file listing `tokens`, dated `modified`.
fn write_token_file(path: &Path, tokens: &[MergeMinedToken], modified: SystemTime) {
    let listed: Vec<String> = tokens
        .iter()
        .map(|token| {
            format!(
                r#"{{"category":"{}","lock_hash":"{}","state_hash":"{}","target_bits":"{:08x}"}}"#,
                hex::encode(token.category),
                hex::encode(token.lock_hash),
                hex::encode(token.state_hash),
                token.target_bits
            )
        })
        .collect();
    let mut file = File::create(path).unwrap();
    file.write_all(format!(r#"{{"tokens":[{}]}}"#, listed.join(",")).as_bytes())
        .unwrap();
    file.set_modified(modified).unwrap();
}

/// The node reads the token file again when it changes, so a claim that
/// moves a token's state reaches the next job; a broken file keeps the last
/// good tokens; a file listing none merge-mines nothing.
#[test]
fn the_token_file_is_read_again_when_it_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("merge-tokens.json");
    let start = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    write_token_file(&path, &[token_a()], start);
    let mut file = TokenFile::open(&path).unwrap();
    assert_eq!(file.current().unwrap().tokens()[0].state_hash, [0xaa; 32]);

    let claimed = MergeMinedToken {
        state_hash: [0xab; 32],
        ..token_a()
    };
    write_token_file(
        &path,
        &[claimed],
        start + std::time::Duration::from_secs(60),
    );
    assert_eq!(file.current().unwrap().tokens()[0].state_hash, [0xab; 32]);

    std::fs::write(&path, b"{").unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(start + std::time::Duration::from_secs(120))
        .unwrap();
    assert_eq!(file.current().unwrap().tokens()[0].state_hash, [0xab; 32]);

    write_token_file(&path, &[], start + std::time::Duration::from_secs(180));
    assert!(file.current().is_none());
}

/// A node does not start on a token file it cannot read or that is not
/// valid.
#[test]
fn a_node_does_not_start_on_a_broken_token_file() {
    let directory = tempfile::tempdir().unwrap();
    assert!(TokenFile::open(directory.path().join("missing.json")).is_err());
    let path = directory.path().join("broken.json");
    std::fs::write(&path, b"{\"tokens\":[{}]}").unwrap();
    assert!(TokenFile::open(&path).is_err());
}

/// These named vectors are `claimTargetBits` outputs of the Node SAFA tool
/// (`tools/safa-auxpow/lib.mjs`, commit `88b696e8`), before `p2poolv2_safa`
/// replaced it. Both implementations must retain its integer rounding.
#[test]
fn safa_claim_target_matches_lib_mjs_one_block_one_thread() {
    assert_eq!(safa_claim_target_bits(0x1d00_ffff, 1, 1), Some(0x1d00_ffff));
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x1d00_ffff, 1, 1),
        Ok(0x1d00_ffff)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_at_easing_cap() {
    assert_eq!(
        safa_claim_target_bits(0x1d00_ffff, 288, 1),
        Some(0x1e00_907f)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x1d00_ffff, 288, 1),
        Ok(0x1e00_907f)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_beyond_easing_cap() {
    assert_eq!(
        safa_claim_target_bits(0x1d00_ffff, 1_000, 63),
        Some(0x1d02_c920)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x1d00_ffff, 1_000, 63),
        Ok(0x1d02_c920)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_maximum_threads() {
    assert_eq!(
        safa_claim_target_bits(0x2000_ffff, 288, 63),
        Some(0x2002_c920)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x2000_ffff, 288, 63),
        Ok(0x2002_c920)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_maximum_target_clamp() {
    assert_eq!(
        safa_claim_target_bits(0x207f_ffff, 288, 1),
        Some(0x207f_ffff)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x207f_ffff, 288, 1),
        Ok(0x207f_ffff)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_minimum_target_clamp() {
    assert_eq!(
        safa_claim_target_bits(0x0300_0001, 1, 63),
        Some(0x0300_0001)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x0300_0001, 1, 63),
        Ok(0x0300_0001)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_fractional_thread_ease() {
    assert_eq!(
        safa_claim_target_bits(0x1a31_06ee, 10, 7),
        Some(0x1a3b_8756)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x1a31_06ee, 10, 7),
        Ok(0x1a3b_8756)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_compact_exponent_carry() {
    assert_eq!(
        safa_claim_target_bits(0x1f7f_ffff, 100, 3),
        Some(0x2008_9553)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x1f7f_ffff, 100, 3),
        Ok(0x2008_9553)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_zero_delta() {
    assert_eq!(safa_claim_target_bits(0x0300_0100, 0, 2), Some(0x0300_0080));
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x0300_0100, 0, 2),
        Ok(0x0300_0080)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_compact_exponent_borrow() {
    assert_eq!(safa_claim_target_bits(0x0400_8000, 0, 1), Some(0x0340_0000));
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x0400_8000, 0, 1),
        Ok(0x0340_0000)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_before_easing_cap() {
    assert_eq!(
        safa_claim_target_bits(0x1c7f_ffff, 287, 5),
        Some(0x1d0e_9999)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x1c7f_ffff, 287, 5),
        Ok(0x1d0e_9999)
    );
}

#[test]
fn safa_claim_target_matches_lib_mjs_zero_delta_at_maximum() {
    assert_eq!(safa_claim_target_bits(0x207f_ffff, 0, 1), Some(0x203f_ffff));
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x207f_ffff, 0, 1),
        Ok(0x203f_ffff)
    );
}

#[test]
fn safa_claim_target_caps_the_height_before_multiplication() {
    assert_eq!(
        safa_claim_target_bits(0x2000_ffff, u32::MAX, 63),
        Some(0x2002_c920)
    );
    assert_eq!(
        p2poolv2_safa::claim_target_bits(0x2000_ffff, u32::MAX, 63),
        Ok(0x2002_c920)
    );
}

#[test]
fn safa_claim_target_rejects_no_threads() {
    assert_eq!(safa_claim_target_bits(0x1d00_ffff, 1, 0), None);
    assert!(p2poolv2_safa::claim_target_bits(0x1d00_ffff, 1, 0).is_err());
}

#[test]
fn safa_claim_target_rejects_too_many_threads() {
    assert_eq!(safa_claim_target_bits(0x1d00_ffff, 1, 64), None);
    assert!(p2poolv2_safa::claim_target_bits(0x1d00_ffff, 1, 64).is_err());
}

#[test]
fn safa_claim_target_rejects_noncanonical_state_bits() {
    assert_eq!(safa_claim_target_bits(0x0200_ffff, 1, 1), None);
    assert!(p2poolv2_safa::claim_target_bits(0x0200_ffff, 1, 1).is_err());
}

/// Every thread count, ease and a spread of state targets, from the
/// hardest to the easiest canonical one: the node and `p2poolv2_safa`
/// agree, clamps and rounding included.
#[test]
fn safa_claim_targets_agree_with_p2poolv2_safa_everywhere() {
    let states = [
        0x0300_0001,
        0x0300_0100,
        0x0300_ffff,
        0x0400_8000,
        0x0240_0000,
        0x1a31_06ee,
        0x1c38_4bd7,
        0x1d00_ffff,
        0x1e00_907f,
        0x1f7f_ffff,
        0x2000_ffff,
        0x2001_0000,
        0x203f_ffff,
        0x207f_ffff,
    ];
    let mut checked = 0;
    for state_bits in states {
        for threads in 1..=SAFA_MAX_THREADS {
            for height_delta in [0, 1, 2, 7, 63, 100, 287, 288, 289, 4_032, u32::MAX] {
                assert_eq!(
                    safa_claim_target_bits(state_bits, height_delta, threads).ok_or(()),
                    p2poolv2_safa::claim_target_bits(state_bits, height_delta, threads)
                        .map_err(|_| ()),
                    "{state_bits:#010x} after {height_delta} blocks over {threads} threads"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 14 * 63 * 11);
}

/// That vault's thread state: thread 5, height 329,000, target
/// `0x2000ffff`, reference bits `0x1a3106ee`.
const SAFA_STATE: &str = "0500000028050500ffff0020ee06311a";

/// A token file naming that vault (category 01..20 as hashed, the chipnet
/// covenant's lock hash) with `threads` and `state`, and `extra` fields.
fn safa_token_json(threads: u32, state: &str, extra: &str) -> String {
    format!(
        r#"{{"tokens":[{{"category":"201f1e1d1c1b1a191817161514131211100f0e0d0c0b0a090807060504030201","lock_hash":"a38bbab2ff9047bceeea12f052cc9a3655178a7f7fd60768716d816fbaad7843","safa":{{"threads":{threads},"state":"{state}"}}{extra}}}]}}"#
    )
}

/// A node mining that vault builds the SAFA tool's leaf at each job's
/// height: the state hash from the thread's state, the target eased by the
/// blocks since it. Checked against the Node tool's printed leaves and
/// against `p2poolv2_safa::claim_leaf` on the compiled chipnet covenant.
#[test]
fn a_safa_vault_s_leaf_follows_its_thread_at_each_height() {
    let set = Arc::new(
        TokenSet::from_json(safa_token_json(63, SAFA_STATE, "").as_bytes())
            .unwrap()
            .unwrap(),
    );
    let split = TokenSplit {
        donation: SplitSlot::new(150, &p2pkh(0xd0)).unwrap(),
        fee: SplitSlot::new(100, &p2pkh(0xfe)).unwrap(),
    };
    assert_eq!(
        hex::encode(set.tokens()[0].state_hash),
        "8811e65fbf63635a1b24f5f09611c25ab1f0021fe40ab06e8156f02cde5a6978"
    );
    let covenant = p2poolv2_safa::create_covenant("chipnet", 63, 42_000_000_000).unwrap();
    assert_eq!(covenant.lock_hash, set.tokens()[0].lock_hash);
    let vault = p2poolv2_safa::Vault {
        category: (1..=32).collect(),
        value: 0,
        token_amount: 0,
        commitment: hex::decode(SAFA_STATE).unwrap(),
    };
    let tool_split = p2poolv2_safa::Split {
        donation_bps: 150,
        donation_locking_bytecode: p2pkh(0xd0),
        fee_bps: 100,
        fee_locking_bytecode: p2pkh(0xfe),
    };
    let next_height = set.at_height(329_001).unwrap();
    assert_eq!(next_height.tokens()[0].target_bits, 0x2000_8205);
    let next_job = next_height.miner_job(&p2pkh(0x12), split);
    assert_eq!(
        hex::encode(next_job.entries[0].leaf.hash()),
        "53e4d65d273996f978a7ef1fcf7235e1961e6fdca2295eab08d18e5471649986"
    );
    let next_tool =
        p2poolv2_safa::claim_leaf(&covenant, &vault, &p2pkh(0x12), Some(&tool_split), 329_001)
            .unwrap();
    assert_eq!(next_tool.target_bits, 0x2000_8205);
    assert_eq!(next_tool.leaf_hash, next_job.entries[0].leaf.hash());
    assert_eq!(
        next_job.entries[0].target,
        expand_token_target(0x2000_8205).unwrap()
    );

    let capped_height = set.at_height(329_300).unwrap();
    assert_eq!(capped_height.tokens()[0].target_bits, 0x2002_c920);
    let capped_job = capped_height.miner_job(&p2pkh(0x12), split);
    assert_eq!(
        hex::encode(capped_job.entries[0].leaf.hash()),
        "e7aaea290fc93691db63a04cd8a5e2495ac9577612e21397a6f30bf73e115789"
    );
    let capped_tool =
        p2poolv2_safa::claim_leaf(&covenant, &vault, &p2pkh(0x12), Some(&tool_split), 329_300)
            .unwrap();
    assert_eq!(capped_tool.target_bits, 0x2002_c920);
    assert_eq!(capped_tool.leaf_hash, capped_job.entries[0].leaf.hash());
    assert_eq!(
        capped_job.entries[0].target,
        expand_token_target(0x2002_c920).unwrap()
    );
    // Preparing a second job reuses the original state rather than easing
    // an already eased target. Old jobs retain the target they committed to.
    assert_eq!(next_height.at_height(329_300).unwrap(), capped_height);
    assert_eq!(set.tokens()[0].target_bits, 0x2000_ffff);
    assert_eq!(next_height.tokens()[0].target_bits, 0x2000_8205);
    // No claim of a coinbase at or below the state's height can succeed.
    assert!(set.at_height(329_000).is_err());
    assert!(set.at_height(1).is_err());
}

/// A set without SAFA tokens is the same at every height.
#[test]
fn a_set_without_safa_tokens_does_not_change_with_height() {
    let set = Arc::new(TokenSet::new(vec![token_a(), token_b()]).unwrap());
    assert!(Arc::ptr_eq(&set, &set.at_height(1).unwrap()));
}

/// A SAFA token gives its thread instead of a state hash and target, with
/// 1 to 63 threads and a 16-byte state whose target is canonical.
#[test]
fn a_safa_token_file_is_checked() {
    assert!(TokenSet::from_json(safa_token_json(1, SAFA_STATE, "").as_bytes()).is_ok());
    assert!(TokenSet::from_json(safa_token_json(0, SAFA_STATE, "").as_bytes()).is_err());
    assert!(TokenSet::from_json(safa_token_json(64, SAFA_STATE, "").as_bytes()).is_err());
    assert!(TokenSet::from_json(safa_token_json(63, "0500", "").as_bytes()).is_err());
    // A state whose target is not canonical token bits.
    assert!(
        TokenSet::from_json(safa_token_json(63, "0500000028050500ffff0021ee06311a", "").as_bytes())
            .is_err()
    );
    assert!(TokenSet::from_json(safa_token_json(
        63,
        SAFA_STATE,
        r#","state_hash":"8811e65fbf63635a1b24f5f09611c25ab1f0021fe40ab06e8156f02cde5a6978""#,
    ).as_bytes()).is_err());
    assert!(
        TokenSet::from_json(
            safa_token_json(63, SAFA_STATE, r#","target_bits":"2000ffff""#,).as_bytes()
        )
        .is_err()
    );
}
