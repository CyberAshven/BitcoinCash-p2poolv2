// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::auxpow::{Disclosure, SplitSlot, TokenSplit, fold};
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::test_utils::test_merge_token_set;
use bitcoin::TxOut;
use bitcoin::consensus::deserialize;

/// P2PKH to twenty `byte` bytes.
fn p2pkh(byte: u8) -> Vec<u8> {
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend([byte; 20]);
    script.extend([0x88, 0xac]);
    script
}

fn split() -> TokenSplit {
    TokenSplit {
        donation: SplitSlot::new(100, &p2pkh(0xd0)).unwrap(),
        fee: SplitSlot::new(100, &p2pkh(0xfe)).unwrap(),
    }
}

fn miner_job() -> MinerTokenJob {
    test_merge_token_set().miner_job(&p2pkh(0x12), split())
}

/// `JobLeaves`: the channel and job, output 0, the length-prefixed `CTMD`
/// that rebuilds output 0's root, and each leaf's branch, which folds the
/// leaf to that root.
#[test]
fn job_leaves_carry_output_0_the_disclosure_and_each_branch() {
    let tokens = miner_job();
    let payload = job_leaves_payload(3, 9, &tokens);

    assert_eq!(payload[..4], 3u32.to_le_bytes());
    assert_eq!(payload[4..8], 9u32.to_le_bytes());
    let output: TxOut = deserialize(&payload[8..61]).unwrap();
    let commitment = MergeMiningCommitment::from_output(&output).unwrap();
    assert_eq!(commitment, tokens.commitment);
    let length = usize::from(u16::from_le_bytes([payload[61], payload[62]]));
    let disclosure = Disclosure::decode(&payload[63..63 + length]).unwrap();
    assert_eq!(disclosure.check_root(&commitment), Ok(()));
    let branches = &payload[63 + length..];
    let depth = 32 * usize::from(commitment.height);
    assert_eq!(branches.len(), depth * disclosure.leaves.len());
    let second = &tokens.entries[1];
    let second_branch: Vec<[u8; 32]> = branches[depth..].as_chunks::<32>().0.to_vec();
    assert_eq!(
        fold(second.leaf.hash(), second.branch.slot, &second_branch),
        commitment.root
    );
}

/// `WinProof`: the channel and job, the category, the length-prefixed
/// proof, then 69 and the split preimage, or 0 without a split.
#[test]
fn a_win_proof_carries_the_proof_and_the_split_the_claim_reveals() {
    let win = TokenWin {
        category: [0x11; 32],
        proof: vec![5; 200],
        split: split(),
    };
    let payload = win_proof_payload(3, 9, &win);
    assert_eq!(payload[..8], [3, 0, 0, 0, 9, 0, 0, 0]);
    assert_eq!(payload[8..40], [0x11; 32]);
    assert_eq!(payload[40..42], 200u16.to_le_bytes());
    assert_eq!(payload[42..242], [5; 200]);
    assert_eq!(payload[242], 69);
    assert_eq!(payload[243..], split().preimage());

    let plain = TokenWin {
        split: TokenSplit::default(),
        ..win
    };
    let payload = win_proof_payload(3, 9, &plain);
    assert_eq!(payload.len(), 243);
    assert_eq!(payload[242], 0);
}

/// The negotiation payloads read back, and a malformed request is refused.
#[test]
fn extension_requests_read_back() {
    assert_eq!(
        requested_extensions(&extensions_payload(4, &[MERGE_DISCLOSURE_EXTENSION, 2])),
        Some((4, vec![MERGE_DISCLOSURE_EXTENSION, 2]))
    );
    assert_eq!(
        extensions_payload(7, &[MERGE_DISCLOSURE_EXTENSION]),
        [7, 0, 1, 0, 0x4d, 0x4d]
    );
    assert_eq!(requested_extensions(&[4, 0, 2, 0, 1]), None);
    assert_eq!(requested_extensions(&[4, 0]), None);
}

/// The SV1 notifications' params: the job id, then hex as Pickaxe's
/// adapter relays them.
#[test]
fn sv1_notifications_carry_hex_params() {
    let tokens = miner_job();
    let leaves = sv1_merge_leaves_params("000000000000002a", &tokens);
    assert_eq!(leaves[0], "000000000000002a");
    assert_eq!(leaves[1], hex::encode(tokens.output()));
    assert_eq!(leaves[2], hex::encode(tokens.disclosure().encode()));
    assert_eq!(leaves[3].as_array().unwrap().len(), 2);

    let win = TokenWin {
        category: [0x11; 32],
        proof: vec![5; 4],
        split: TokenSplit::default(),
    };
    let proof = sv1_merge_proof_params("000000000000002a", &win);
    assert_eq!(proof[1], hex::encode([0x11; 32]));
    assert_eq!(proof[2], "05050505");
    assert_eq!(proof[3], "");
}

/// The log writer appends each line, and past its size moves the file to
/// `.1` and starts again.
#[test]
fn a_line_log_appends_and_moves_aside_when_full() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("merge-tokens.leaves.jsonl");
    let (log, writer) = LineLog::start(path.clone(), 100);
    log.write("a".repeat(60));
    log.write("b".repeat(60));
    log.write("c".repeat(10));
    drop(log);
    writer.join().unwrap();

    let rotated =
        std::fs::read_to_string(directory.path().join("merge-tokens.leaves.jsonl.1")).unwrap();
    assert_eq!(rotated, format!("{}\n{}\n", "a".repeat(60), "b".repeat(60)));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{}\n", "c".repeat(10))
    );
}

/// A standard channel's job and a win each make one JSON line, naming no
/// address.
#[test]
fn the_logs_write_one_json_line_per_job_and_win() {
    let directory = tempfile::tempdir().unwrap();
    let (leaves, leaves_writer) =
        LineLog::start(directory.path().join("leaves.jsonl"), MAX_LOG_BYTES);
    let (wins, wins_writer) = LineLog::start(directory.path().join("wins.jsonl"), MAX_LOG_BYTES);
    let logs = MergeMiningLogs::from_logs(leaves, wins);
    let tokens = miner_job();
    let win = TokenWin {
        category: [0x11; 32],
        proof: vec![5; 4],
        split: split(),
    };
    logs.log_standard_job(3, 9, &tokens);
    logs.log_sv2_win(3, 9, &tokens, &win);
    logs.log_sv1_win("000000000000002a", &tokens, &win);
    drop(logs);
    leaves_writer.join().unwrap();
    wins_writer.join().unwrap();

    let leaves = std::fs::read_to_string(directory.path().join("leaves.jsonl")).unwrap();
    let job: Value = serde_json::from_str(leaves.trim_end()).unwrap();
    assert_eq!(job["channel"], 3);
    assert_eq!(job["job"], 9);
    assert_eq!(job["output0"], hex::encode(tokens.output()));
    assert_eq!(job["leaves"], hex::encode(tokens.disclosure().encode()));
    assert!(job["time"].as_u64().is_some());

    let wins = std::fs::read_to_string(directory.path().join("wins.jsonl")).unwrap();
    let lines: Vec<Value> = wins
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["channel"], 3);
    assert_eq!(
        lines[0]["payout_hash"],
        hex::encode(tokens.entries[0].leaf.payout_hash)
    );
    assert_eq!(lines[0]["proof"], "05050505");
    assert_eq!(lines[0]["split"], hex::encode(split().preimage()));
    assert_eq!(lines[1]["job"], "000000000000002a");
    assert!(lines[1].get("channel").is_none());
}
