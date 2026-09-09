//! Bounded, priority write queue in front of one PTY master.

use parking_lot::{Condvar, Mutex};
use std::collections::VecDeque;

/// Input bytes a terminal may hold before further writes are dropped. Well past
/// any realistic paste; beyond it the child has stopped draining its PTY and the
/// only alternative is unbounded growth.
const MAX_QUEUED_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// Query replies are tens of bytes each, so this only bounds a query storm
/// against a child that stopped reading.
const MAX_QUEUED_RESPONSE_BYTES: usize = 256 * 1024;

/// Input bytes per `write_all`. Caps how long a reply queued mid-paste waits to
/// one chunk instead of the whole backlog.
const MAX_INPUT_BATCH_BYTES: usize = 64 * 1024;

/// Outcome of queueing a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Queued {
    Ok,
    /// The lane is full; the chunk was dropped whole rather than truncated.
    Full,
    /// The PTY is being torn down.
    Closed,
}

#[derive(Default)]
struct QueueState {
    responses: VecDeque<u8>,
    input: VecDeque<u8>,
    closed: bool,
}

/// Write queue for one PTY, drained by the writer thread as the single IO owner.
///
/// Producers only ever push, so the authoritative reactor never waits on a PTY
/// write. Terminal→program replies use a priority lane: a query answer must not
/// sit behind a paste the child has not drained yet.
pub(crate) struct PtyWriteQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl PtyWriteQueue {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(QueueState::default()),
            ready: Condvar::new(),
        }
    }

    /// Queue program input behind anything already pending.
    pub(crate) fn push_input(&self, data: &[u8]) -> Queued {
        self.push(data, false)
    }

    /// Queue a terminal→program reply ahead of any pending input.
    pub(crate) fn push_response(&self, data: &[u8]) -> Queued {
        self.push(data, true)
    }

    fn push(&self, data: &[u8], response: bool) -> Queued {
        if data.is_empty() {
            return Queued::Ok;
        }
        let mut state = self.state.lock();
        if state.closed {
            return Queued::Closed;
        }
        let (lane, cap) = if response {
            (&mut state.responses, MAX_QUEUED_RESPONSE_BYTES)
        } else {
            (&mut state.input, MAX_QUEUED_INPUT_BYTES)
        };
        if lane.len() + data.len() > cap {
            return Queued::Full;
        }
        lane.extend(data.iter().copied());
        drop(state);
        self.ready.notify_one();
        Queued::Ok
    }

    /// Block until something is queued, then take the next write. Replies are
    /// drained whole and first, so no batch can split one around input bytes.
    /// `None` once the queue is closed and drained.
    pub(crate) fn next_batch(&self) -> Option<Vec<u8>> {
        let mut state = self.state.lock();
        while state.responses.is_empty() && state.input.is_empty() {
            if state.closed {
                return None;
            }
            self.ready.wait(&mut state);
        }
        let mut batch = Vec::with_capacity(state.responses.len() + MAX_INPUT_BATCH_BYTES);
        batch.extend(state.responses.drain(..));
        let take = MAX_INPUT_BATCH_BYTES
            .saturating_sub(batch.len())
            .min(state.input.len());
        batch.extend(state.input.drain(..take));
        Some(batch)
    }

    /// Stop accepting writes and wake the writer thread. Idempotent and never
    /// blocking: both teardown and the `Drop` backstop call it.
    pub(crate) fn close(&self) {
        self.state.lock().closed = true;
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn drain_all(queue: &PtyWriteQueue) -> Vec<Vec<u8>> {
        queue.close();
        std::iter::from_fn(|| queue.next_batch()).collect()
    }

    #[test]
    fn a_reply_is_written_before_input_queued_before_it() {
        let queue = PtyWriteQueue::new();
        assert_eq!(queue.push_input(b"paste"), Queued::Ok);
        assert_eq!(queue.push_response(b"\x1b[0n"), Queued::Ok);

        assert_eq!(
            queue.next_batch().as_deref(),
            Some(b"\x1b[0npaste".as_ref())
        );
    }

    #[test]
    fn a_reply_queued_mid_paste_precedes_the_rest_of_the_backlog() {
        let queue = PtyWriteQueue::new();
        assert_eq!(
            queue.push_input(&vec![b'x'; MAX_INPUT_BATCH_BYTES * 3]),
            Queued::Ok
        );

        let first = queue.next_batch().expect("first chunk");
        assert_eq!(first.len(), MAX_INPUT_BATCH_BYTES);

        assert_eq!(queue.push_response(b"\x1b[0n"), Queued::Ok);
        let second = queue.next_batch().expect("second chunk");
        assert!(
            second.starts_with(b"\x1b[0n"),
            "a reply must not wait for the whole paste to drain"
        );
        assert_eq!(second.len(), MAX_INPUT_BATCH_BYTES);
    }

    #[test]
    fn input_past_the_cap_is_dropped_whole() {
        let queue = PtyWriteQueue::new();
        assert_eq!(
            queue.push_input(&vec![b'x'; MAX_QUEUED_INPUT_BYTES]),
            Queued::Ok
        );
        assert_eq!(queue.push_input(b"one more byte"), Queued::Full);

        let queued: usize = drain_all(&queue).iter().map(Vec::len).sum();
        assert_eq!(queued, MAX_QUEUED_INPUT_BYTES);
    }

    #[test]
    fn replies_past_the_cap_are_dropped_whole() {
        let queue = PtyWriteQueue::new();
        assert_eq!(
            queue.push_response(&vec![b'r'; MAX_QUEUED_RESPONSE_BYTES]),
            Queued::Ok
        );
        assert_eq!(queue.push_response(b"\x1b[0n"), Queued::Full);

        let queued: usize = drain_all(&queue).iter().map(Vec::len).sum();
        assert_eq!(queued, MAX_QUEUED_RESPONSE_BYTES);
    }

    #[test]
    fn close_drains_what_is_queued_and_then_ends() {
        let queue = PtyWriteQueue::new();
        assert_eq!(queue.push_input(b"tail"), Queued::Ok);
        queue.close();

        assert_eq!(queue.next_batch().as_deref(), Some(b"tail".as_ref()));
        assert_eq!(queue.next_batch(), None);
        assert_eq!(queue.push_input(b"after close"), Queued::Closed);
    }

    #[test]
    fn a_waiting_writer_wakes_on_close() {
        let queue = Arc::new(PtyWriteQueue::new());
        let waiter = {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || queue.next_batch())
        };
        queue.close();
        assert_eq!(waiter.join().expect("waiter finishes"), None);
    }
}
