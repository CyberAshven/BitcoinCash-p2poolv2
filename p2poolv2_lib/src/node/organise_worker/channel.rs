// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The organise channel, bounded by event count and by bytes.
//!
//! A tokio channel bounds only how many events it queues. An
//! `OrganiseEvent::Block` carries a whole `ShareBlock`, up to
//! `BCH_SHARE_BODY_SIZE_LIMIT` (1,204,800 bytes) on Bitcoin Cash, so the
//! channel also holds a byte budget: a sender takes as many permits as the
//! block's consensus encoding has bytes before it queues the event, and
//! waits while the queue holds the whole budget. The permits travel with
//! the event and go back when the organise worker receives it. That waiting
//! is the backpressure: it holds the validation task, and so its validation
//! permit, until the organise worker catches up.
//!
//! A block larger than the whole budget takes the whole budget, so it still
//! passes once the queue is empty. `InvalidBlock` carries only a hash and
//! takes no bytes.

use super::OrganiseEvent;
use std::sync::Arc;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

/// An event on the channel, with the bytes it holds of the budget.
struct QueuedEvent {
    event: OrganiseEvent,
    /// Released when the receiver takes the event.
    _bytes: Option<OwnedSemaphorePermit>,
}

/// Sender half of the organise channel.
#[derive(Clone)]
pub struct OrganiseSender {
    events: mpsc::Sender<QueuedEvent>,
    bytes: Arc<Semaphore>,
    /// The whole byte budget, the most one event takes.
    max_bytes: u32,
}

/// Receiver half of the organise channel.
pub struct OrganiseReceiver {
    events: mpsc::Receiver<QueuedEvent>,
    bytes: Arc<Semaphore>,
}

/// A channel of at most `capacity` events and `max_bytes` bytes of share
/// blocks. A budget over `u32::MAX` bytes is `u32::MAX`.
pub(super) fn organise_channel(
    capacity: usize,
    max_bytes: usize,
) -> (OrganiseSender, OrganiseReceiver) {
    let max_bytes = u32::try_from(max_bytes).unwrap_or(u32::MAX);
    let bytes = Arc::new(Semaphore::new(max_bytes as usize));
    let (events_tx, events_rx) = mpsc::channel(capacity);
    (
        OrganiseSender {
            events: events_tx,
            bytes: Arc::clone(&bytes),
            max_bytes,
        },
        OrganiseReceiver {
            events: events_rx,
            bytes,
        },
    )
}

/// Bytes `event` holds on the channel: a block's consensus encoding.
fn event_bytes(event: &OrganiseEvent) -> usize {
    match event {
        OrganiseEvent::Block { share_block, .. } => share_block.encoded_size(),
        OrganiseEvent::InvalidBlock(_) => 0,
    }
}

impl OrganiseSender {
    /// Queue `event`, waiting while the channel holds its most events or
    /// its byte budget. Fails, returning the event, once the receiver is
    /// gone.
    // The error hands the event back, as `mpsc::Sender::send` does; it is
    // as large as the event and only built once the worker has stopped.
    #[allow(clippy::result_large_err)]
    pub async fn send(&self, event: OrganiseEvent) -> Result<(), SendError<OrganiseEvent>> {
        let size = event_bytes(&event);
        let permit = if size == 0 {
            None
        } else {
            let permits =
                u32::try_from(size).map_or(self.max_bytes, |size| size.min(self.max_bytes));
            match Arc::clone(&self.bytes).acquire_many_owned(permits).await {
                Ok(permit) => Some(permit),
                Err(_) => return Err(SendError(event)),
            }
        };
        self.events
            .send(QueuedEvent {
                event,
                _bytes: permit,
            })
            .await
            .map_err(|SendError(queued)| SendError(queued.event))
    }

    /// Bytes of the budget free now.
    #[cfg(test)]
    pub(super) fn available_bytes(&self) -> usize {
        self.bytes.available_permits()
    }
}

impl OrganiseReceiver {
    /// The next event, or `None` once every sender is gone and the queue is
    /// empty. Taking an event returns its bytes to the budget.
    pub async fn recv(&mut self) -> Option<OrganiseEvent> {
        self.events.recv().await.map(|queued| queued.event)
    }
}

impl Drop for OrganiseReceiver {
    /// Wake any sender waiting for bytes, so it fails as a send to a closed
    /// channel does instead of waiting forever.
    fn drop(&mut self) {
        self.bytes.close();
    }
}

#[cfg(test)]
mod tests;
