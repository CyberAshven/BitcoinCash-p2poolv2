// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stop a sim swarm. Replaces `load-tests/sim/stop-swarm.sh`.
//!
//! Returns only once the node processes have actually exited. Callers read
//! the RocksDB stores right afterwards, and a node that has logged its
//! shutdown but not yet exited still holds the store lock: every read-write
//! open then fails, which looks like an empty or corrupt store rather than a
//! stop that was not waited for.

use crate::process::{ProcessTable, Signal, find_by_command_line, send_signal};
use crate::sim::RunDir;
use std::time::Duration;

/// Default `STOP_GRACE_SECONDS`.
pub const DEFAULT_GRACE_SECONDS: u64 = 10;

/// Node processes still running for this swarm. Children of this process
/// that have exited are reaped first, so they do not linger as zombies.
fn running_nodes(run_dir: &RunDir, processes: &mut ProcessTable) -> Vec<u32> {
    processes.reap_exited();
    find_by_command_line(&run_dir.process_pattern())
}

/// Poll once a second until no node process remains, for up to `seconds`.
async fn wait_for_exit(run_dir: &RunDir, processes: &mut ProcessTable, seconds: u64) -> bool {
    for _ in 0..seconds {
        if running_nodes(run_dir, processes).is_empty() {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    running_nodes(run_dir, processes).is_empty()
}

fn joined(pids: &[u32]) -> String {
    pids.iter().map(|pid| format!("{pid} ")).collect()
}

/// Stop every node of the swarm in `run_dir`: SIGTERM to the PIDs in
/// `pids.txt` and to any orphan whose command line names a config in
/// `run_dir`, then SIGKILL after `grace_seconds`. True when every node
/// process has exited.
pub async fn stop_swarm(run_dir: &RunDir, processes: &mut ProcessTable, grace_seconds: u64) -> bool {
    let signalled = run_dir
        .read_pids()
        .into_iter()
        .flatten()
        .filter(|pid| send_signal(&[*pid], Signal::Terminate))
        .count();

    // Also catch orphans this run directory spawned that are not in
    // pids.txt, such as a prior run whose pids file was overwritten.
    let orphans = running_nodes(run_dir, processes);
    send_signal(&orphans, Signal::Terminate);

    let mut stopped = true;
    if !wait_for_exit(run_dir, processes, grace_seconds).await {
        let survivors = running_nodes(run_dir, processes);
        println!(
            "Nodes still running after {grace_seconds}s, sending SIGKILL: {}",
            joined(&survivors)
        );
        send_signal(&survivors, Signal::Kill);
        if !wait_for_exit(run_dir, processes, 5).await {
            eprintln!(
                "WARNING: nodes survived SIGKILL and still hold their store locks: {}",
                joined(&running_nodes(run_dir, processes))
            );
            stopped = false;
        }
    }
    println!(
        "Stopped swarm (pids.txt: {signalled}; all node processes for {} have exited).",
        run_dir.path().display()
    );
    stopped
}
