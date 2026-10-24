//! Pickaxe's golden vectors for merge-mining v2 (pickaxe-miner PR #70,
//! commit `a1c80a1ba6ad6b1542af54f4af28058d3241c3df` on
//! `feat/merge-mining-v2`: `docs/merge-mining.md` and
//! `src/stratum_v2/merge/{tree,commitment,proof,verify,split,vectors}.rs`), then
//! P2Pool's own cases.

use super::*;
use crate::test_utils::test_bch_auxpow;
use bitcoin::CompactTarget;

/// P2PKH to twenty 0x12 bytes: the vectors' payout.
fn vector_payout() -> Vec<u8> {
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend([0x12; 20]);
    script.extend([0x88, 0xac]);
    script
}

fn token_a() -> TokenLeaf {
    TokenLeaf::new(
        [0x11; 32],
        &[0x51],
        [0xaa; 32],
        &vector_payout(),
        0x207f_ffff,
        [0; 32],
    )
}

fn token_b() -> TokenLeaf {
    TokenLeaf::new(
        [0x22; 32],
        &[0x52],
        [0xbb; 32],
        &vector_payout(),
        0x1f00_ffff,
        [0; 32],
    )
}

const LEAF_A: &str = concat!(
    "43544d4c02111111111111111111111111111111111111111111111111111111",
    "1111111111953ccfa596a6c6d39e5980194539124fdcff116a571455a212baed",
    "811f585ee0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaa22650758a42ebfd7306e771ad24c442cad795ed9e549392233729f",
    "647a2b1be5ffff7f200000000000000000000000000000000000000000000000",
    "000000000000000000",
);
const LEAF_A_HASH: &str = "0ab882d24f8c85b880ca7cfb6f60a7773f377222e84ac28afd14f3ce46bf307a";
const LEAF_B_HASH: &str = "9520e74e37e5dffaef19eb2f61da54e502d296c8f1baa3f5dc7db772711def59";
const TWO_TOKEN_ROOT: &str = "5b61ecfe96c714397bc35ed813890f49f2b0287a78111e19d156f4eecd1c38d0";
const TWO_TOKEN_OUTPUT: &str = concat!(
    "00000000000000002c6a2a43544d4d025b61ecfe96c714397bc35ed813890f49",
    "f2b0287a78111e19d156f4eecd1c38d00100000000",
);

/// The 224-byte one-leaf proof: token A alone, `h = 0`, `d = 1`.
const PROOF: &str = concat!(
    "43544d5002000000205555555555555555555555555555555555555555555555",
    "5555555555555555556c933baf900d7d82a08e5689a968f70b77802a116c671c",
    "63c12b9ed534e67a1d00f15365ffff7f200000000000000000003b0002000000",
    "0100000000000000000000000000000000000000000000000000000000000000",
    "00ffffffff0c0315f9040707070707070707ffffffff022600205fa012000000",
    "001976a914121212121212121212121212121212121212121288ac0000000001",
    "4444444444444444444444444444444444444444444444444444444444444444",
);
const PROOF_COINBASE_TXID: &str =
    "245dbc4c7d02cc87b5a41eb8af9f8aebf36ffa528864177931fb3b3d527f35e9";

/// The first byte where two encodings differ, for a readable failure.
fn first_mismatch(left: &[u8], right: &[u8]) -> Option<usize> {
    left.iter()
        .zip(right)
        .position(|(a, b)| a != b)
        .or((left.len() != right.len()).then(|| left.len().min(right.len())))
}

#[test]
fn golden_leaf_preimage_and_hashes() {
    let (a, b) = (token_a(), token_b());
    assert_eq!(
        first_mismatch(&a.preimage(), &hex::decode(LEAF_A).unwrap()),
        None
    );
    assert_eq!(hex::encode(a.hash()), LEAF_A_HASH);
    assert_eq!(hex::encode(b.hash()), LEAF_B_HASH);
}

#[test]
fn golden_two_token_tree() {
    let (a, b) = (token_a(), token_b());
    let tree = AuxiliaryTree::build(&[a, b]).unwrap();
    // The smallest height that holds two leaves, and the first salt that
    // parts them: slots 0 and 1.
    assert_eq!((tree.commitment.height, tree.commitment.salt), (1, 0));
    assert_eq!(
        tree.branches
            .iter()
            .map(|branch| branch.slot)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(hex::encode(tree.commitment.root), TWO_TOKEN_ROOT);
    assert_eq!(serialize(&tree.commitment.output()).len(), 53);
    assert_eq!(
        hex::encode(serialize(&tree.commitment.output())),
        TWO_TOKEN_OUTPUT
    );
    assert_eq!(slot(&a.category, 0, 1), tree.branches[0].slot);
    assert_eq!(slot(&b.category, 0, 1), tree.branches[1].slot);
    assert_eq!(
        fold(a.hash(), tree.branches[0].slot, &tree.branches[0].branch),
        tree.commitment.root
    );
    assert_eq!(
        fold(b.hash(), tree.branches[1].slot, &tree.branches[1].branch),
        tree.commitment.root
    );
    // The same tokens in the other order give the same commitment.
    assert_eq!(
        AuxiliaryTree::build(&[b, a]).unwrap().commitment,
        tree.commitment
    );
}

/// The synthetic coinbase around output 0: head and tail.
fn vector_coinbase(commitment: MergeMiningCommitment) -> Transaction {
    let mut bytes = 2u32.to_le_bytes().to_vec();
    bytes.extend(NULL_INPUT);
    // L = 12: the BIP34 height 325,909 and eight extranonce bytes.
    let script: Vec<u8> = [&[0x03, 0x15, 0xf9, 0x04][..], &[0x07; 8]].concat();
    bytes.push(script.len() as u8);
    bytes.extend(script);
    bytes.extend(u32::MAX.to_le_bytes());
    bytes.push(2);
    bytes.extend(serialize(&commitment.output()));
    bytes.extend(312_500_000u64.to_le_bytes());
    bytes.push(25);
    bytes.extend(vector_payout());
    bytes.extend(0u32.to_le_bytes());
    deserialize(&bytes).unwrap()
}

/// The one-leaf proof, built here from the same parts, encodes to Pickaxe's
/// 224 bytes, and Pickaxe's bytes verify for token A.
#[test]
fn golden_one_leaf_proof_encodes_and_verifies() {
    let leaf = token_a();
    let tree = AuxiliaryTree::build(&[leaf]).unwrap();
    assert_eq!((tree.commitment.height, tree.commitment.salt), (0, 0));
    assert_eq!(tree.commitment.root, leaf.hash());
    let coinbase = vector_coinbase(tree.commitment);
    assert_eq!(
        hex::encode(coinbase.compute_txid().to_byte_array()),
        PROOF_COINBASE_TXID
    );
    let sibling = TxMerkleNode::from_byte_array([0x44; 32]);
    let merkle_root = TxMerkleNode::from_byte_array(hash256(
        &[
            coinbase.compute_txid().to_byte_array(),
            sibling.to_byte_array(),
        ]
        .concat(),
    ));
    let header = Header {
        version: bitcoin::block::Version::from_consensus(0x2000_0000),
        prev_blockhash: bitcoin::BlockHash::from_byte_array([0x55; 32]),
        merkle_root,
        time: 1_700_000_000,
        bits: CompactTarget::from_consensus(0x207f_ffff),
        nonce: 0,
    };
    let proof = AuxProof::new(
        header,
        &coinbase,
        &[sibling],
        tree.commitment,
        &tree.branches[0],
    )
    .unwrap();
    let bytes = proof.to_bytes().unwrap();
    let golden = hex::decode(PROOF).unwrap();
    assert_eq!(
        bytes,
        golden,
        "first differing proof byte: {:?}",
        first_mismatch(&bytes, &golden)
    );
    assert_eq!(bytes.len(), 224);

    let decoded = AuxProof::from_bytes(&golden).unwrap();
    assert_eq!(decoded, proof);
    assert_eq!(decoded.coinbase_height(), Some(325_909));
    decoded
        .verify(&leaf, MAX_MERGE_MINING_TREE_HEIGHT, None)
        .unwrap();
    // The header's regtest bits are above BCH's limit: the mainnet ASERT
    // check refuses them.
    assert!(matches!(
        decoded.verify(&leaf, MAX_MERGE_MINING_TREE_HEIGHT, Some(BchAsert::MAINNET)),
        Err(AuxError::Asert(_))
    ));
}

/// The proof commits to all 32 state bytes; an entirely different state
/// changes the reconstructed coinbase rather than selecting another claim.
#[test]
fn golden_proof_rejects_another_state_hash() {
    let proof = AuxProof::from_bytes(&hex::decode(PROOF).unwrap()).unwrap();
    let stale = TokenLeaf {
        state_hash: [0xab; 32],
        ..token_a()
    };
    assert_eq!(
        proof.verify(&stale, MAX_MERGE_MINING_TREE_HEIGHT, None),
        Err(AuxError::MerkleRoot)
    );
}

#[test]
fn golden_proof_rejects_a_changed_first_state_byte() {
    let proof = AuxProof::from_bytes(&hex::decode(PROOF).unwrap()).unwrap();
    let mut stale = token_a();
    stale.state_hash[0] ^= 1;
    assert_eq!(
        proof.verify(&stale, MAX_MERGE_MINING_TREE_HEIGHT, None),
        Err(AuxError::MerkleRoot)
    );
}

#[test]
fn golden_proof_rejects_a_changed_last_state_byte() {
    let proof = AuxProof::from_bytes(&hex::decode(PROOF).unwrap()).unwrap();
    let mut stale = token_a();
    stale.state_hash[31] ^= 1;
    assert_eq!(
        proof.verify(&stale, MAX_MERGE_MINING_TREE_HEIGHT, None),
        Err(AuxError::MerkleRoot)
    );
}

#[test]
fn proof_decoding_is_canonical() {
    let golden = hex::decode(PROOF).unwrap();
    let mut trailing = golden.clone();
    trailing.push(0);
    assert_eq!(AuxProof::from_bytes(&trailing), Err(AuxError::Decode));
    assert_eq!(
        AuxProof::from_bytes(&golden[..golden.len() - 1]),
        Err(AuxError::Decode)
    );
    for offset in [0, 4] {
        let mut changed = golden.clone();
        changed[offset] ^= 1;
        assert_eq!(AuxProof::from_bytes(&changed), Err(AuxError::Decode));
    }
    // h above 16, and a coinbase branch deeper than 25.
    let mut tall = golden.clone();
    tall[85] = 17;
    assert_eq!(AuxProof::from_bytes(&tall), Err(AuxError::Decode));
    let mut proof = AuxProof::from_bytes(&golden).unwrap();
    proof.coinbase_branch = vec![[0x44; 32]; MAX_AUX_PROOF_COINBASE_BRANCH_LENGTH + 1];
    assert_eq!(proof.to_bytes(), Err(AuxError::Shape));
}

#[test]
fn verifier_checks_height_target_head_and_tail() {
    let golden = AuxProof::from_bytes(&hex::decode(PROOF).unwrap()).unwrap();
    let leaf = token_a();
    let mut token_limit = leaf;
    token_limit.target_bits = 0x2080_0000;
    assert_eq!(golden.verify(&token_limit, 16, None), Err(AuxError::Target));
    let mut proof = golden.clone();
    proof.coinbase_head[4] = 2;
    assert_eq!(proof.verify(&leaf, 16, None), Err(AuxError::CoinbaseHead));
    let mut proof = golden.clone();
    proof.coinbase_tail.truncate(3);
    assert_eq!(proof.verify(&leaf, 16, None), Err(AuxError::CoinbaseTail));
    // A harder token target than the header's work.
    let mut hard = leaf;
    hard.target_bits = 0x0300_0001;
    let proof = AuxProof {
        header: golden.header,
        ..golden.clone()
    };
    assert!(proof.verify(&hard, 16, None).is_err());
    // A two-leaf proof is refused by a token limited to one leaf.
    let tree = AuxiliaryTree::build(&[token_a(), token_b()]).unwrap();
    let coinbase = vector_coinbase(tree.commitment);
    let header = Header {
        merkle_root: TxMerkleNode::from_byte_array(coinbase.compute_txid().to_byte_array()),
        ..golden.header
    };
    let two = AuxProof::new(header, &coinbase, &[], tree.commitment, &tree.branches[0]).unwrap();
    assert_eq!(two.verify(&leaf, 0, None), Err(AuxError::Height));
}

#[test]
fn builder_limits_tokens_and_categories() {
    assert_eq!(
        AuxiliaryTree::build(&[]).unwrap().commitment,
        MergeMiningCommitment::empty()
    );
    assert_eq!(
        AuxiliaryTree::build(&[token_a(), token_a()]),
        Err(AuxError::DuplicateCategory)
    );
    let many: Vec<TokenLeaf> = (0..=MAX_TOKEN_LEAVES as u8)
        .map(|index| TokenLeaf {
            category: [index; 32],
            ..token_a()
        })
        .collect();
    assert_eq!(AuxiliaryTree::build(&many), Err(AuxError::TooManyTokens));
    let sixteen = AuxiliaryTree::build(&many[..MAX_TOKEN_LEAVES]).unwrap();
    assert!(sixteen.commitment.height >= 4);
    for (leaf, branch) in many.iter().zip(&sixteen.branches) {
        assert_eq!(
            fold(leaf.hash(), branch.slot, &branch.branch),
            sixteen.commitment.root
        );
    }
}

#[test]
fn token_targets_must_be_canonical_compact() {
    assert!(expand_token_target(0x207f_ffff).is_some());
    assert!(expand_token_target(0x0300_0001).is_some());
    for invalid in [0x0200_0001, 0x2100_0001, 0x2080_0000, 0x1d00_0000] {
        assert_eq!(expand_token_target(invalid), None, "{invalid:#x}");
    }
    let target = expand_token_target(0x1d00_ffff).unwrap();
    assert_eq!(&target[26..28], &[0xff, 0xff]);
    assert!(
        target[..26]
            .iter()
            .chain(&target[28..])
            .all(|byte| *byte == 0)
    );
}

/// A block P2Pool itself builds for a declared Bitcoin Cash job, with a
/// two-token commitment at output 0, proves either token's win.
#[test]
fn proof_from_a_p2pool_declared_coinbase_verifies() {
    let (proof, leaf) = test_bch_auxpow();
    proof.verify(&leaf, 16, None).unwrap();
    let bytes = proof.to_bytes().unwrap();
    assert_eq!(AuxProof::from_bytes(&bytes).unwrap(), proof);
    let other = TokenLeaf {
        category: [0x99; 32],
        ..leaf
    };
    assert!(proof.verify(&other, 16, None).is_err());
}

/// P2PKH to twenty `byte` bytes.
fn p2pkh(byte: u8) -> Vec<u8> {
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend([byte; 20]);
    script.extend([0x88, 0xac]);
    script
}

const SPLIT_PREIMAGE: &str = concat!(
    "0364008bc613f574964583c7b08b7925373515b513ed15b372bc727599c16434",
    "254d1c6400543bb5ec93f33a1c27a5822e4c638e9ab1e90e44c094fd7aaba51e",
    "fc4e554c50",
);
const SPLIT_EXT: &str = "79f31f395015b16c76e12673ec8f58329d81f1884d76a0d210d1f3158a8faa85";
const SPLIT_LEAF: &str = concat!(
    "43544d4c02111111111111111111111111111111111111111111111111111111",
    "1111111111953ccfa596a6c6d39e5980194539124fdcff116a571455a212baed",
    "811f585ee0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaa22650758a42ebfd7306e771ad24c442cad795ed9e549392233729f",
    "647a2b1be5ffff7f2079f31f395015b16c76e12673ec8f58329d81f1884d76a0",
    "d210d1f3158a8faa85",
);
const SPLIT_LEAF_HASH: &str = "c40271368b4287df8fe3430e1bc3f9410b554874de698144fa04fe5df7033e41";

/// Pickaxe's whole split leaf (`vectors.rs`, `a_whole_split_leaf_and_its_claim_outputs`):
/// token A with a 1% donation to P2PKH(d0 x 20) and a 1% fee to
/// P2PKH(fe x 20). Its 69-byte preimage, `ext`, 169-byte leaf and leaf hash
/// match byte for byte, and the preimage parses back.
#[test]
fn golden_split_leaf() {
    let split = TokenSplit {
        donation: SplitSlot::new(100, &p2pkh(0xd0)).unwrap(),
        fee: SplitSlot::new(100, &p2pkh(0xfe)).unwrap(),
    };
    assert_eq!(
        first_mismatch(&split.preimage(), &hex::decode(SPLIT_PREIMAGE).unwrap()),
        None
    );
    assert_eq!(split.preimage().len(), 69);
    assert_eq!(hex::encode(split.ext()), SPLIT_EXT);
    let leaf = TokenLeaf::new(
        [0x11; 32],
        &[0x51],
        [0xaa; 32],
        &vector_payout(),
        0x207f_ffff,
        split.ext(),
    );
    assert_eq!(
        first_mismatch(&leaf.preimage(), &hex::decode(SPLIT_LEAF).unwrap()),
        None
    );
    assert_eq!(hex::encode(leaf.hash()), SPLIT_LEAF_HASH);
    assert_eq!(TokenSplit::parse(&split.preimage()), Ok(split));
}

/// The split is canonical: no split is the zero `ext` and never a
/// preimage; a rate is at most 10,000; a zero hash exactly when the rate is
/// 0; exactly 69 bytes starting with 0x03.
#[test]
fn token_split_is_canonical() {
    assert_eq!(TokenSplit::default().ext(), [0; 32]);
    assert_eq!(
        TokenSplit::parse(&TokenSplit::default().preimage()),
        Err(AuxError::Split)
    );
    assert_eq!(SplitSlot::new(0, &p2pkh(0xd0)), Ok(SplitSlot::EMPTY));
    assert!(SplitSlot::new(MAX_SPLIT_BPS, &p2pkh(0xd0)).is_ok());
    assert_eq!(
        SplitSlot::new(MAX_SPLIT_BPS + 1, &p2pkh(0xd0)),
        Err(AuxError::Split)
    );
    assert_eq!(SplitSlot::from_hash(1, [0; 32]), Err(AuxError::Split));
    assert_eq!(SplitSlot::from_hash(0, [1; 32]), Err(AuxError::Split));
    let preimage = hex::decode(SPLIT_PREIMAGE).unwrap();
    assert_eq!(TokenSplit::parse(&preimage[..68]), Err(AuxError::Split));
    let mut prefix = preimage.clone();
    prefix[0] = 0x02;
    assert_eq!(TokenSplit::parse(&prefix), Err(AuxError::Split));
    let mut over = preimage;
    over[1..3].copy_from_slice(&10_001u16.to_le_bytes());
    assert_eq!(TokenSplit::parse(&over), Err(AuxError::Split));
    // A donation alone is a split too.
    let donation_only = TokenSplit {
        donation: SplitSlot::new(150, &p2pkh(0xd0)).unwrap(),
        fee: SplitSlot::EMPTY,
    };
    assert_ne!(donation_only.ext(), [0; 32]);
    assert_eq!(
        TokenSplit::parse(&donation_only.preimage()),
        Ok(donation_only)
    );
}
