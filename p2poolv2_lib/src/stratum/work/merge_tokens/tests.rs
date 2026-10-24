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
    }
}

fn token_b() -> MergeMinedToken {
    MergeMinedToken {
        category: [0x22; 32],
        lock_hash: bitcoin::hashes::sha256d::Hash::hash(&[0x52]).to_byte_array(),
        state_hash: [0xbb; 32],
        target_bits: 0x1f00_ffff,
        max_height: MAX_MERGE_MINING_TREE_HEIGHT,
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
