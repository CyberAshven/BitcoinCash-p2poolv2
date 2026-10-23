use super::*;
use crate::test_utils::test_bch_auxpow;
use bitcoin::{Amount, OutPoint, ScriptBuf, Witness};

#[test]
fn two_chains_match_an_independently_hashed_tree() {
    let first = AuxiliaryWork {
        chain_id: 1,
        hash: sha256d::Hash::hash(b"token-A"),
    };
    let second = AuxiliaryWork {
        chain_id: 2,
        hash: sha256d::Hash::hash(b"token-B"),
    };
    let tree = AuxiliaryTree::build(&[first, second], 0).unwrap();
    assert_eq!(auxiliary_slot(1, 0, 2), 1);
    assert_eq!(auxiliary_slot(2, 0, 2), 0);
    assert_eq!(tree.commitment.tree_size, 2);
    assert_eq!(
        hex::encode(tree.commitment.merkle_root),
        "d37b6e24f7874f8e72d5d05f41c80935b2aa0b58bddf767a65848fbc88e576d0"
    );
    tree.proofs[0].verify(tree.commitment, first).unwrap();
    tree.proofs[1].verify(tree.commitment, second).unwrap();
    assert_eq!(
        AuxiliaryTree::build(&[second, first], 0)
            .unwrap()
            .commitment,
        tree.commitment
    );
}

#[test]
fn one_chain_reverses_the_root_and_empty_selection_reserves_the_output() {
    let work = AuxiliaryWork {
        chain_id: u32::MAX,
        hash: sha256d::Hash::hash(b"token-A"),
    };
    let tree = AuxiliaryTree::build(&[work], u32::MAX).unwrap();
    assert_eq!(tree.commitment.tree_size, 1);
    assert_eq!(
        hex::encode(tree.commitment.merkle_root),
        work.hash.to_string()
    );
    assert!(tree.proofs[0].branch.is_empty());
    tree.proofs[0].verify(tree.commitment, work).unwrap();
    let empty = AuxiliaryTree::build(&[], 99).unwrap();
    assert_eq!(empty.commitment, MergeMiningCommitment::reserved());
    assert!(empty.proofs.is_empty());
}

#[test]
fn slot_collisions_grow_the_tree_without_nonce_retry() {
    let first = AuxiliaryWork {
        chain_id: 1,
        hash: sha256d::Hash::hash(b"token-A"),
    };
    let second = AuxiliaryWork {
        chain_id: 3,
        hash: sha256d::Hash::hash(b"token-B"),
    };
    let tree = AuxiliaryTree::build(&[first, second], 9).unwrap();
    assert_eq!(tree.commitment.tree_size, 4);
    assert_eq!(tree.proofs[0].branch.len(), 2);
    tree.proofs[0].verify(tree.commitment, first).unwrap();
    tree.proofs[1].verify(tree.commitment, second).unwrap();
}

#[test]
fn sparse_maximum_width_does_not_allocate_a_billion_leaves() {
    let first = AuxiliaryWork {
        chain_id: 0,
        hash: sha256d::Hash::hash(b"token-A"),
    };
    let second = AuxiliaryWork {
        chain_id: 1 << 29,
        hash: sha256d::Hash::hash(b"token-B"),
    };
    let tree = AuxiliaryTree::build(&[first, second], u32::MAX).unwrap();
    assert_eq!(tree.commitment.tree_size, 1 << 30);
    assert_eq!(tree.proofs[0].branch.len(), 30);
    tree.proofs[0].verify(tree.commitment, first).unwrap();
    tree.proofs[1].verify(tree.commitment, second).unwrap();
}

#[test]
fn duplicate_ids_and_permanent_slot_collisions_are_rejected() {
    let first = AuxiliaryWork {
        chain_id: 1,
        hash: sha256d::Hash::hash(b"token-A"),
    };
    let mut second = AuxiliaryWork {
        chain_id: 1,
        hash: sha256d::Hash::hash(b"token-B"),
    };
    assert!(
        AuxiliaryTree::build(&[first, second], 0)
            .unwrap_err()
            .contains("duplicate")
    );
    second.chain_id += 1 << 30;
    assert!(
        AuxiliaryTree::build(&[first, second], 0)
            .unwrap_err()
            .contains("collide")
    );
}

#[test]
fn work_selection_budget_is_enforced() {
    let work: Vec<_> = (0..MAX_AUXILIARY_CHAINS as u32)
        .map(|chain_id| AuxiliaryWork {
            chain_id,
            hash: sha256d::Hash::all_zeros(),
        })
        .collect();
    assert_eq!(
        AuxiliaryTree::build(&work, 0).unwrap().proofs.len(),
        MAX_AUXILIARY_CHAINS
    );
    let mut excessive = work;
    excessive.push(AuxiliaryWork {
        chain_id: 100,
        hash: sha256d::Hash::all_zeros(),
    });
    assert!(AuxiliaryTree::build(&excessive, 0).is_err());
}

#[test]
fn auxiliary_proof_binds_identity_depth_nonce_and_hash_order() {
    let (proof, expected) = test_bch_auxpow();
    let commitment = MergeMiningCommitment::from_output(&proof.coinbase.output[2]).unwrap();
    proof.auxiliary.verify(commitment, expected).unwrap();
    let wrong = AuxiliaryWork {
        chain_id: 9,
        ..expected
    };
    assert!(proof.auxiliary.verify(commitment, wrong).is_err());
    let mut changed = proof.auxiliary.clone();
    changed.branch[0] = sha256d::Hash::hash(b"wrong sibling");
    assert!(changed.verify(commitment, expected).is_err());
    changed = proof.auxiliary.clone();
    changed.branch.push(sha256d::Hash::all_zeros());
    assert!(changed.verify(commitment, expected).is_err());
    assert!(
        proof
            .auxiliary
            .verify(
                MergeMiningCommitment {
                    nonce: 1,
                    ..commitment
                },
                expected
            )
            .is_err()
    );
    let mut reversed = commitment;
    reversed.merkle_root.reverse();
    assert!(proof.auxiliary.verify(reversed, expected).is_err());
}

#[test]
fn full_proof_checks_trusted_target_and_parent_merkle_root() {
    let (mut proof, expected) = test_bch_auxpow();
    let target = proof.parent_header.target();
    proof.verify(expected, target).unwrap();
    assert!(proof.verify(expected, Target::ZERO).is_err());
    let mut minimum_target = [0; 32];
    minimum_target[31] = 1;
    assert!(
        proof
            .verify(expected, Target::from_be_bytes(minimum_target))
            .is_err()
    );
    proof.parent_header.merkle_root = TxMerkleNode::all_zeros();
    assert!(
        proof
            .verify(expected, target)
            .unwrap_err()
            .contains("parent header")
    );
}

#[test]
fn full_proof_rejects_duplicate_and_paid_commitments() {
    let (mut proof, expected) = test_bch_auxpow();
    let target = proof.parent_header.target();
    proof.coinbase.output.push(proof.coinbase.output[2].clone());
    assert!(
        proof
            .verify(expected, target)
            .unwrap_err()
            .contains("multiple")
    );
    proof.coinbase.output.pop();
    proof.coinbase.output[2].value = Amount::from_sat(1);
    assert!(proof.verify(expected, target).is_err());
}

#[test]
fn full_proof_rejects_non_coinbase_witness_and_resource_overruns() {
    let (mut proof, expected) = test_bch_auxpow();
    let target = proof.parent_header.target();
    proof.coinbase.input[0].previous_output.vout = 0;
    assert!(proof.verify(expected, target).is_err());
    proof.coinbase.input[0].previous_output = OutPoint::null();
    proof.coinbase.input[0].witness = Witness::from_slice(&[b"witness"]);
    assert!(proof.verify(expected, target).is_err());
    proof.coinbase.input[0].witness = Witness::new();
    proof.coinbase_branch = vec![TxMerkleNode::all_zeros(); 26];
    assert!(proof.verify(expected, target).is_err());
    proof.coinbase_branch.clear();
    proof.coinbase.output[0].script_pubkey =
        ScriptBuf::from_bytes(vec![0; MAX_AUXPOW_COINBASE_SIZE]);
    assert!(proof.verify(expected, target).is_err());
}
