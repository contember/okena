//! Prompts waiting for the pane's MCP channel server (`okena claude-remote
//! channel`), which long-polls for them.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

/// The channel still counts as attached this long after a poll returned, so a
/// prompt sent while the server is between two polls goes to the channel.
const ATTACH_GRACE: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct ChannelState {
    prompts: VecDeque<String>,
    /// The outstanding poll, if any. A newer poll finishes an older one.
    waiter: Option<ChannelWaiter>,
    last_poll_end: Option<Instant>,
}

struct ChannelWaiter {
    poll_id: u64,
    respond: oneshot::Sender<Vec<String>>,
}

pub(super) enum PollStart {
    Ready(Vec<String>),
    Wait(oneshot::Receiver<Vec<String>>),
}

impl ChannelState {
    pub fn is_attached(&self, now: Instant) -> bool {
        self.waiter.is_some()
            || self
                .last_poll_end
                .is_some_and(|end| now.saturating_duration_since(end) <= ATTACH_GRACE)
    }

    /// Queue `prompt` and hand everything queued to the outstanding poll.
    pub fn push(&mut self, prompt: String, now: Instant) {
        self.prompts.push_back(prompt);
        let Some(waiter) = self.waiter.take() else {
            return;
        };
        let prompts: Vec<String> = self.prompts.drain(..).collect();
        match waiter.respond.send(prompts) {
            Ok(()) => self.last_poll_end = Some(now),
            // The poll went away in the meantime; keep the prompts for the next.
            Err(prompts) => self.prompts = prompts.into(),
        }
    }

    pub fn begin_poll(&mut self, poll_id: u64, now: Instant) -> PollStart {
        if let Some(older) = self.waiter.take() {
            let _ = older.respond.send(Vec::new());
        }
        if !self.prompts.is_empty() {
            self.last_poll_end = Some(now);
            return PollStart::Ready(self.prompts.drain(..).collect());
        }
        let (respond, receive) = oneshot::channel();
        self.waiter = Some(ChannelWaiter { poll_id, respond });
        PollStart::Wait(receive)
    }

    /// The poll `poll_id` returned or was dropped. A poll that is no longer
    /// the outstanding one was already answered and timed.
    pub fn end_poll(&mut self, poll_id: u64, now: Instant) {
        if self
            .waiter
            .as_ref()
            .is_some_and(|waiter| waiter.poll_id == poll_id)
        {
            self.waiter = None;
            self.last_poll_end = Some(now);
        }
    }

    /// The session ended: its channel server exits with it.
    pub fn detach(&mut self) {
        if let Some(waiter) = self.waiter.take() {
            let _ = waiter.respond.send(Vec::new());
        }
        self.last_poll_end = None;
    }

    pub fn clear_prompts(&mut self) {
        self.prompts.clear();
    }

    pub fn queued(&self) -> usize {
        self.prompts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attached_while_polling_and_for_the_grace_period_after() {
        let start = Instant::now();
        let mut channel = ChannelState::default();
        assert!(!channel.is_attached(start));

        let PollStart::Wait(_receive) = channel.begin_poll(1, start) else {
            panic!("nothing queued, the poll must wait");
        };
        assert!(channel.is_attached(start + Duration::from_secs(60)));

        channel.end_poll(1, start);
        assert!(channel.is_attached(start + Duration::from_secs(10)));
        assert!(!channel.is_attached(start + Duration::from_secs(11)));
    }

    #[test]
    fn detach_ignores_the_late_end_of_the_ended_poll() {
        let now = Instant::now();
        let mut channel = ChannelState::default();
        let PollStart::Wait(_receive) = channel.begin_poll(1, now) else {
            panic!("nothing queued, the poll must wait");
        };

        channel.detach();
        channel.end_poll(1, now);

        assert!(!channel.is_attached(now));
    }

    #[test]
    fn push_wakes_the_waiting_poll() {
        let now = Instant::now();
        let mut channel = ChannelState::default();
        let PollStart::Wait(mut receive) = channel.begin_poll(1, now) else {
            panic!("nothing queued, the poll must wait");
        };

        channel.push("hello".into(), now);

        assert_eq!(
            receive.try_recv().expect("woken"),
            vec!["hello".to_string()]
        );
        assert_eq!(channel.queued(), 0);
        assert!(channel.is_attached(now));
    }

    #[test]
    fn prompts_queued_between_polls_are_returned_at_once() {
        let now = Instant::now();
        let mut channel = ChannelState::default();
        channel.push("a".into(), now);
        channel.push("b".into(), now);

        let PollStart::Ready(prompts) = channel.begin_poll(1, now) else {
            panic!("queued prompts must be returned immediately");
        };
        assert_eq!(prompts, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn newer_poll_finishes_the_older_one_empty() {
        let now = Instant::now();
        let mut channel = ChannelState::default();
        let PollStart::Wait(mut older) = channel.begin_poll(1, now) else {
            panic!("must wait");
        };
        let PollStart::Wait(mut newer) = channel.begin_poll(2, now) else {
            panic!("must wait");
        };
        assert_eq!(older.try_recv().expect("finished"), Vec::<String>::new());

        // The older poll ending must not detach the newer one.
        channel.end_poll(1, now);
        channel.push("x".into(), now);
        assert_eq!(newer.try_recv().expect("woken"), vec!["x".to_string()]);
    }

    #[test]
    fn push_to_a_dropped_poll_keeps_the_prompt() {
        let now = Instant::now();
        let mut channel = ChannelState::default();
        let PollStart::Wait(receive) = channel.begin_poll(1, now) else {
            panic!("must wait");
        };
        drop(receive);

        channel.push("kept".into(), now);
        assert_eq!(channel.queued(), 1);
    }
}
