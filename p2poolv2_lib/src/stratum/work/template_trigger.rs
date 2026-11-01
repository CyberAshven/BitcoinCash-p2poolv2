// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Wake-ups for the template task besides its poll.
//!
//! The template task fetches a new block template on a ZMQ `hashblock` event,
//! on a block the node announces on its P2P port (with `node_p2p_address`),
//! or when its poll interval elapses. None of them is sure to fire when this
//! node itself finds a block: the node's ZMQ endpoint and P2P port can be
//! unreachable (a remote node), and the poll can be up to `GBT_POLL_INTERVAL`
//! away. Until then every miner keeps working on the parent of the block just
//! found, so any further block it finds is a same-height sibling that loses.
//!
//! [`merge_template_triggers`] adds a refresh channel beside the ZMQ one (and
//! the node P2P one, when configured) and forwards them all into the single
//! receiver the template task already reads.

use tokio::sync::mpsc;

/// Merge the ZMQ trigger, and the node P2P trigger if there is one, with a
/// refresh channel the stratum servers signal after submitting a found block.
///
/// Returns the refresh sender, for the stratum servers, and the merged
/// receiver, for `start_gbt`. All channels hold one pending wake-up, so a
/// burst of requests while a template is being fetched becomes one fetch.
/// With both ZMQ and the node P2P trigger, one block wakes each, which costs
/// at most one extra fetch. The forwarder ends when the template task drops
/// the merged receiver, or when all inputs are closed.
pub fn merge_template_triggers(
    mut zmq_trigger_rx: mpsc::Receiver<()>,
    node_p2p_trigger_rx: Option<mpsc::Receiver<()>>,
) -> (mpsc::Sender<()>, mpsc::Receiver<()>) {
    let (refresh_tx, mut refresh_rx) = mpsc::channel(1);
    let (trigger_tx, trigger_rx) = mpsc::channel(1);
    tokio::spawn(async move {
        let mut zmq_open = true;
        let mut refresh_open = true;
        let mut node_p2p_open = node_p2p_trigger_rx.is_some();
        // Without a node P2P trigger, a receiver that is never polled.
        let mut node_p2p_trigger_rx = node_p2p_trigger_rx.unwrap_or_else(|| mpsc::channel(1).1);
        while zmq_open || refresh_open || node_p2p_open {
            let wake = tokio::select! {
                event = zmq_trigger_rx.recv(), if zmq_open => {
                    zmq_open = event.is_some();
                    event
                }
                event = refresh_rx.recv(), if refresh_open => {
                    refresh_open = event.is_some();
                    event
                }
                event = node_p2p_trigger_rx.recv(), if node_p2p_open => {
                    node_p2p_open = event.is_some();
                    event
                }
            };
            if wake.is_some() && trigger_tx.send(()).await.is_err() {
                break;
            }
        }
    });
    (refresh_tx, trigger_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    async fn next(trigger_rx: &mut mpsc::Receiver<()>) -> Option<()> {
        timeout(Duration::from_secs(2), trigger_rx.recv())
            .await
            .expect("a trigger arrives")
    }

    #[tokio::test]
    async fn a_refresh_request_wakes_the_template_task() {
        let (_zmq_tx, zmq_rx) = mpsc::channel(1);
        let (refresh_tx, mut trigger_rx) = merge_template_triggers(zmq_rx, None);
        refresh_tx.try_send(()).unwrap();
        assert_eq!(next(&mut trigger_rx).await, Some(()));
    }

    #[tokio::test]
    async fn a_zmq_event_still_wakes_the_template_task() {
        let (zmq_tx, zmq_rx) = mpsc::channel(1);
        let (_refresh_tx, mut trigger_rx) = merge_template_triggers(zmq_rx, None);
        zmq_tx.send(()).await.unwrap();
        assert_eq!(next(&mut trigger_rx).await, Some(()));
    }

    #[tokio::test]
    async fn a_node_p2p_announcement_wakes_the_template_task() {
        let (_zmq_tx, zmq_rx) = mpsc::channel(1);
        let (node_p2p_tx, node_p2p_rx) = mpsc::channel(1);
        let (_refresh_tx, mut trigger_rx) = merge_template_triggers(zmq_rx, Some(node_p2p_rx));
        node_p2p_tx.send(()).await.unwrap();
        assert_eq!(next(&mut trigger_rx).await, Some(()));
    }

    #[tokio::test]
    async fn refreshes_keep_working_after_zmq_closes() {
        let (zmq_tx, zmq_rx) = mpsc::channel(1);
        let (refresh_tx, mut trigger_rx) = merge_template_triggers(zmq_rx, None);
        drop(zmq_tx);
        refresh_tx.try_send(()).unwrap();
        assert_eq!(next(&mut trigger_rx).await, Some(()));
    }

    #[tokio::test]
    async fn a_burst_of_requests_is_coalesced() {
        let (_zmq_tx, zmq_rx) = mpsc::channel(1);
        let (refresh_tx, mut trigger_rx) = merge_template_triggers(zmq_rx, None);
        for _ in 0..10 {
            let _ = refresh_tx.try_send(());
        }
        assert_eq!(next(&mut trigger_rx).await, Some(()));
        // At most one request is in the refresh channel and one in the
        // forwarder, so the burst yields at most two wake-ups, not ten.
        let mut extra = 0;
        while timeout(Duration::from_millis(200), trigger_rx.recv())
            .await
            .is_ok()
        {
            extra += 1;
        }
        assert!(extra <= 1, "{extra} extra wake-ups");
    }

    #[tokio::test]
    async fn the_merged_receiver_closes_when_both_inputs_close() {
        let (zmq_tx, zmq_rx) = mpsc::channel(1);
        let (refresh_tx, mut trigger_rx) = merge_template_triggers(zmq_rx, None);
        drop(zmq_tx);
        drop(refresh_tx);
        assert_eq!(next(&mut trigger_rx).await, None);
    }
}
