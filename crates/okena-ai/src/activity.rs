//! In-memory record of the last hour of `/v1/messages` traffic, for the
//! activity overlay: which account answered, failovers and failures.

use okena_core::ai_proxy::{
    AI_ACTIVITY_MINUTES, AiActivity, AiActivityEvent, AiActivityKind, AiRouteActivity,
};
use parking_lot::Mutex;
use std::collections::{BTreeMap, VecDeque};

const MINUTE_MS: i64 = 60 * 1000;
const WINDOW_MS: i64 = AI_ACTIVITY_MINUTES as i64 * MINUTE_MS;
const MAX_EVENTS: usize = 50;
const MAX_MESSAGE_CHARS: usize = 300;

enum Outcome {
    Answered { error: bool },
    RateLimited,
}

struct Record {
    at_ms: i64,
    /// `None`: the terminal's own Claude login.
    account_id: Option<String>,
    outcome: Outcome,
}

#[derive(Default)]
struct Log {
    records: VecDeque<Record>,
    /// Newest first.
    events: VecDeque<AiActivityEvent>,
}

impl Log {
    fn record(&mut self, now: i64, account_id: Option<&str>, outcome: Outcome) {
        while self
            .records
            .front()
            .is_some_and(|record| record.at_ms <= now - WINDOW_MS)
        {
            self.records.pop_front();
        }
        self.records.push_back(Record {
            at_ms: now,
            account_id: account_id.map(str::to_owned),
            outcome,
        });
    }

    fn event(&mut self, now: i64, project_id: Option<&str>, kind: AiActivityKind) {
        let repeated = self
            .events
            .iter()
            .position(|event| event.kind == kind && event.project_id.as_deref() == project_id)
            .and_then(|index| self.events.remove(index));
        let event = match repeated {
            Some(event) => AiActivityEvent {
                at_ms: now,
                count: event.count + 1,
                ..event
            },
            None => AiActivityEvent {
                at_ms: now,
                project_id: project_id.map(str::to_owned),
                kind,
                count: 1,
            },
        };
        self.events.push_front(event);
        self.events.truncate(MAX_EVENTS);
    }

    fn snapshot(&self, now: i64, since_ms: i64) -> AiActivity {
        let mut routes: BTreeMap<Option<String>, AiRouteActivity> = BTreeMap::new();
        for record in &self.records {
            let age = now - record.at_ms;
            if !(0..WINDOW_MS).contains(&age) {
                continue;
            }
            let route =
                routes
                    .entry(record.account_id.clone())
                    .or_insert_with(|| AiRouteActivity {
                        account_id: record.account_id.clone(),
                        per_minute: vec![0; AI_ACTIVITY_MINUTES],
                        errors: 0,
                        rate_limited: 0,
                    });
            match record.outcome {
                Outcome::Answered { error } => {
                    let minutes_ago = (age / MINUTE_MS) as usize;
                    route.per_minute[AI_ACTIVITY_MINUTES - 1 - minutes_ago] += 1;
                    route.errors += u32::from(error);
                }
                Outcome::RateLimited => route.rate_limited += 1,
            }
        }
        AiActivity {
            since_ms,
            routes: routes.into_values().collect(),
            events: self.events.iter().cloned().collect(),
        }
    }
}

pub struct Activity {
    started_ms: i64,
    log: Mutex<Log>,
}

impl Activity {
    pub fn new() -> Self {
        Self {
            started_ms: crate::now_ms(),
            log: Mutex::default(),
        }
    }

    /// A pool account (or, with `account_id: None`, the terminal's own login)
    /// answered with `status`; `message` is the error message of an error answer.
    pub fn answered(
        &self,
        project_id: Option<&str>,
        account_id: Option<&str>,
        status: u16,
        message: Option<String>,
    ) {
        let now = crate::now_ms();
        let error = status >= 400;
        let mut log = self.log.lock();
        log.record(now, account_id, Outcome::Answered { error });
        if error {
            log.event(
                now,
                project_id,
                AiActivityKind::Failed {
                    account_id: account_id.map(str::to_owned),
                    status: Some(status),
                    message: message.map(truncate_message),
                },
            );
        }
    }

    /// The provider could not be reached through `account_id`, or stayed silent.
    pub fn unanswered(&self, project_id: Option<&str>, account_id: Option<&str>, message: String) {
        let now = crate::now_ms();
        let mut log = self.log.lock();
        log.record(now, account_id, Outcome::Answered { error: true });
        log.event(
            now,
            project_id,
            AiActivityKind::Failed {
                account_id: account_id.map(str::to_owned),
                status: None,
                message: Some(truncate_message(message)),
            },
        );
    }

    pub fn rate_limited(&self, project_id: Option<&str>, account_id: &str) {
        let now = crate::now_ms();
        let mut log = self.log.lock();
        log.record(now, Some(account_id), Outcome::RateLimited);
        log.event(
            now,
            project_id,
            AiActivityKind::RateLimited {
                account_id: account_id.to_owned(),
            },
        );
    }

    /// An event that is not tied to an answer: rejected token, no usable account.
    pub fn event(&self, project_id: Option<&str>, kind: AiActivityKind) {
        self.log.lock().event(crate::now_ms(), project_id, kind);
    }

    pub fn snapshot(&self) -> AiActivity {
        self.log.lock().snapshot(crate::now_ms(), self.started_ms)
    }
}

fn truncate_message(message: String) -> String {
    match message.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((end, _)) => format!("{}…", &message[..end]),
        None => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 10 * WINDOW_MS;

    fn answered(log: &mut Log, at_ms: i64, account_id: Option<&str>, error: bool) {
        log.record(at_ms, account_id, Outcome::Answered { error });
    }

    #[test]
    fn answers_land_in_the_minute_they_happened() {
        let mut log = Log::default();
        answered(&mut log, NOW - 10_000, Some("a"), false);
        answered(&mut log, NOW - 30_000, Some("a"), true);
        answered(&mut log, NOW - 61_000, Some("a"), false);
        answered(&mut log, NOW - WINDOW_MS + 1, Some("a"), false);
        log.record(NOW - 5_000, Some("a"), Outcome::RateLimited);
        answered(&mut log, NOW - 1_000, None, false);

        let activity = log.snapshot(NOW, 7);
        assert_eq!(activity.since_ms, 7);
        let [own, a] = activity.routes.as_slice() else {
            panic!("expected two routes, got {:?}", activity.routes);
        };
        assert_eq!(own.account_id, None);
        assert_eq!(own.per_minute[AI_ACTIVITY_MINUTES - 1], 1);
        assert_eq!(a.account_id.as_deref(), Some("a"));
        assert_eq!(a.per_minute[AI_ACTIVITY_MINUTES - 1], 2);
        assert_eq!(a.per_minute[AI_ACTIVITY_MINUTES - 2], 1);
        assert_eq!(a.per_minute[0], 1);
        assert_eq!(a.per_minute.iter().sum::<u32>(), 4);
        assert_eq!(a.errors, 1);
        assert_eq!(a.rate_limited, 1);
    }

    #[test]
    fn records_older_than_an_hour_are_dropped() {
        let mut log = Log::default();
        answered(&mut log, NOW - WINDOW_MS, Some("old"), false);
        answered(&mut log, NOW, Some("new"), false);
        assert_eq!(log.records.len(), 1);
        let activity = log.snapshot(NOW + WINDOW_MS, 0);
        assert!(activity.routes.is_empty());
    }

    #[test]
    fn a_repeated_event_moves_to_the_top_and_the_list_is_capped() {
        let mut log = Log::default();
        log.event(1, Some("p"), AiActivityKind::AllRateLimited);
        log.event(2, Some("q"), AiActivityKind::AllRateLimited);
        log.event(3, Some("p"), AiActivityKind::AllRateLimited);
        let events = log.snapshot(3, 0).events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].project_id.as_deref(), Some("p"));
        assert_eq!((events[0].count, events[0].at_ms), (2, 3));
        assert_eq!(events[1].project_id.as_deref(), Some("q"));

        for at_ms in 0..2 * MAX_EVENTS as i64 {
            let project = at_ms.to_string();
            log.event(at_ms, Some(&project), AiActivityKind::NoPermittedAccount);
        }
        assert_eq!(log.events.len(), MAX_EVENTS);
    }

    #[test]
    fn long_messages_are_cut_on_a_char_boundary() {
        let long = "ž".repeat(MAX_MESSAGE_CHARS + 5);
        let cut = truncate_message(long);
        assert_eq!(cut.chars().count(), MAX_MESSAGE_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(truncate_message("short".into()), "short");
    }
}
