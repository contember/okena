//! Opt-in terminal input latency instrumentation.
//!
//! Build with `terminal-latency-probe`, then set
//! `OKENA_TERMINAL_LATENCY_PROBE=1` before starting Okena. The desktop and its
//! daemon then record matching per-terminal samples. The controlled test must
//! keep one input in flight and avoid unrelated terminal output.

#[cfg(feature = "terminal-latency-probe")]
mod imp {
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    const MAX_PENDING_PER_TERMINAL: usize = 512;

    #[derive(Debug, Clone)]
    struct ClientProbe {
        sample: u64,
        viewer: u64,
        input_len: usize,
        input_hash: u64,
        input_us: u64,
        output_receive_us: Option<u64>,
        parsed_us: Option<u64>,
        activity_emit_us: Option<u64>,
        activity_receive_us: Option<u64>,
        throttle_fire_us: Option<u64>,
        notify_us: Option<u64>,
        paint_us: Option<u64>,
    }

    #[derive(Debug, Clone)]
    struct DaemonProbe {
        sample: u64,
        input_len: usize,
        input_hash: u64,
        ws_receive_us: u64,
        bridge_us: Option<u64>,
        pty_queue_us: Option<u64>,
        pty_write_start_us: Option<u64>,
        pty_write_end_us: Option<u64>,
        pty_output_us: Option<u64>,
        stream_us: Option<u64>,
    }

    #[derive(Default)]
    struct ProbeBook {
        next_client_sample: HashMap<String, u64>,
        next_daemon_sample: HashMap<String, u64>,
        client: HashMap<String, VecDeque<ClientProbe>>,
        daemon: HashMap<String, VecDeque<DaemonProbe>>,
    }

    impl ProbeBook {
        fn start_client(
            &mut self,
            terminal_id: &str,
            viewer: u64,
            data: &[u8],
            now_us: u64,
        ) -> u64 {
            let sample = next_sample(&mut self.next_client_sample, terminal_id);
            let pending = self.client.entry(terminal_id.to_string()).or_default();
            trim_pending(pending);
            pending.push_back(ClientProbe {
                sample,
                viewer,
                input_len: data.len(),
                input_hash: input_hash(data),
                input_us: now_us,
                output_receive_us: None,
                parsed_us: None,
                activity_emit_us: None,
                activity_receive_us: None,
                throttle_fire_us: None,
                notify_us: None,
                paint_us: None,
            });
            sample
        }

        fn start_daemon(&mut self, terminal_id: &str, data: &[u8], now_us: u64) -> u64 {
            let sample = next_sample(&mut self.next_daemon_sample, terminal_id);
            let pending = self.daemon.entry(terminal_id.to_string()).or_default();
            trim_pending(pending);
            pending.push_back(DaemonProbe {
                sample,
                input_len: data.len(),
                input_hash: input_hash(data),
                ws_receive_us: now_us,
                bridge_us: None,
                pty_queue_us: None,
                pty_write_start_us: None,
                pty_write_end_us: None,
                pty_output_us: None,
                stream_us: None,
            });
            sample
        }

        fn mark_client(
            &mut self,
            terminal_id: &str,
            now_us: u64,
            update: impl Fn(&mut ClientProbe, u64),
        ) {
            if let Some(pending) = self.client.get_mut(terminal_id) {
                for probe in pending {
                    update(probe, now_us);
                }
            }
        }

        fn mark_daemon(
            &mut self,
            terminal_id: &str,
            now_us: u64,
            update: impl Fn(&mut DaemonProbe, u64),
        ) {
            if let Some(pending) = self.daemon.get_mut(terminal_id) {
                for probe in pending {
                    update(probe, now_us);
                }
            }
        }

        fn client_painted(&mut self, terminal_id: &str, viewer: u64, now_us: u64) -> Vec<u64> {
            let Some(pending) = self.client.get_mut(terminal_id) else {
                return Vec::new();
            };
            let mut painted = Vec::new();
            for probe in pending {
                if probe.viewer == viewer && probe.parsed_us.is_some() && probe.paint_us.is_none() {
                    probe.paint_us = Some(now_us);
                    painted.push(probe.sample);
                }
            }
            painted
        }

        fn client_framed(
            &mut self,
            terminal_id: &str,
            viewer: u64,
            samples: &[u64],
        ) -> Vec<ClientProbe> {
            let Some(mut pending) = self.client.remove(terminal_id) else {
                return Vec::new();
            };
            let mut completed = Vec::new();
            pending.retain(|probe| {
                if probe.viewer == viewer && samples.contains(&probe.sample) {
                    completed.push(probe.clone());
                    false
                } else {
                    true
                }
            });
            if !pending.is_empty() {
                self.client.insert(terminal_id.to_string(), pending);
            }
            completed
        }

        fn daemon_streamed(&mut self, terminal_id: &str) -> Vec<DaemonProbe> {
            let Some(mut pending) = self.daemon.remove(terminal_id) else {
                return Vec::new();
            };
            let mut completed = Vec::new();
            pending.retain(|probe| {
                if probe.pty_write_end_us.is_some()
                    && probe.pty_output_us.is_some()
                    && probe.stream_us.is_some()
                {
                    completed.push(probe.clone());
                    false
                } else {
                    true
                }
            });
            if !pending.is_empty() {
                self.daemon.insert(terminal_id.to_string(), pending);
            }
            completed
        }
    }

    static ENABLED: OnceLock<bool> = OnceLock::new();
    static PROBES: OnceLock<Mutex<ProbeBook>> = OnceLock::new();

    pub fn enabled() -> bool {
        *ENABLED.get_or_init(|| {
            std::env::var("OKENA_TERMINAL_LATENCY_PROBE")
                .ok()
                .is_some_and(|value| !matches!(value.as_str(), "" | "0" | "false" | "off"))
        })
    }

    pub fn client_start(terminal_id: &str, viewer: u64, data: &[u8]) {
        with_book(|book| {
            let sample = book.start_client(terminal_id, viewer, data, now_us());
            log::info!(
                target: "okena::terminal_latency",
                "terminal_latency_client_start terminal={terminal_id} sample={sample} viewer={viewer}"
            );
        });
    }

    pub fn client_output_received(terminal_id: &str) {
        mark_client(terminal_id, |probe, now| {
            probe.output_receive_us.get_or_insert(now);
        });
    }

    pub fn client_output_parsed(terminal_id: &str) {
        mark_client(terminal_id, |probe, now| {
            if probe.output_receive_us.is_some() {
                probe.parsed_us.get_or_insert(now);
            }
        });
    }

    pub fn client_activity_emitted(terminal_id: &str) {
        mark_client(terminal_id, |probe, now| {
            if probe.parsed_us.is_some() {
                probe.activity_emit_us.get_or_insert(now);
            }
        });
    }

    pub fn client_activity_received(terminal_id: &str) {
        mark_client(terminal_id, |probe, now| {
            if probe.activity_emit_us.is_some() {
                probe.activity_receive_us.get_or_insert(now);
            }
        });
    }

    pub fn client_repaint_dispatched(terminal_id: &str) {
        mark_client(terminal_id, |probe, now| {
            if probe.activity_receive_us.is_some() {
                probe.throttle_fire_us.get_or_insert(now);
            }
        });
    }

    pub fn client_notify_requested(terminal_id: &str, viewer: u64) {
        mark_client(terminal_id, |probe, now| {
            if probe.viewer == viewer && probe.throttle_fire_us.is_some() {
                probe.notify_us.get_or_insert(now);
            }
        });
    }

    pub fn client_painted(terminal_id: &str, viewer: u64) -> Vec<u64> {
        if !enabled() {
            return Vec::new();
        }
        lock_book()
            .map(|mut book| book.client_painted(terminal_id, viewer, now_us()))
            .unwrap_or_default()
    }

    pub fn client_frame_completed(terminal_id: &str, viewer: u64, samples: &[u64]) {
        if !enabled() {
            return;
        }
        let frame_us = now_us();
        let completed = lock_book()
            .map(|mut book| book.client_framed(terminal_id, viewer, samples))
            .unwrap_or_default();
        for probe in completed {
            log::info!(
                target: "okena::terminal_latency",
                "terminal_latency_client terminal={terminal_id} sample={} viewer={} input_len={} input_hash={} input_us={} output_receive_us={} parsed_us={} activity_emit_us={} activity_receive_us={} throttle_fire_us={} notify_us={} paint_us={} frame_us={frame_us}",
                probe.sample,
                probe.viewer,
                probe.input_len,
                probe.input_hash,
                probe.input_us,
                value(probe.output_receive_us),
                value(probe.parsed_us),
                value(probe.activity_emit_us),
                value(probe.activity_receive_us),
                value(probe.throttle_fire_us),
                value(probe.notify_us),
                value(probe.paint_us),
            );
        }
    }

    pub fn daemon_input_received(terminal_id: &str, data: &[u8]) {
        with_book(|book| {
            book.start_daemon(terminal_id, data, now_us());
        });
    }

    pub fn daemon_bridge_received(terminal_id: &str) {
        mark_daemon(terminal_id, |probe, now| {
            probe.bridge_us.get_or_insert(now);
        });
    }

    pub fn daemon_pty_queued(terminal_id: &str) {
        mark_daemon(terminal_id, |probe, now| {
            if probe.bridge_us.is_some() {
                probe.pty_queue_us.get_or_insert(now);
            }
        });
    }

    pub fn daemon_pty_write_started(terminal_id: &str) {
        mark_daemon(terminal_id, |probe, now| {
            if probe.pty_queue_us.is_some() {
                probe.pty_write_start_us.get_or_insert(now);
            }
        });
    }

    pub fn daemon_pty_write_completed(terminal_id: &str) {
        mark_daemon(terminal_id, |probe, now| {
            if probe.pty_write_start_us.is_some() {
                probe.pty_write_end_us.get_or_insert(now);
            }
        });
        log_completed_daemon(terminal_id);
    }

    pub fn daemon_pty_output_received(terminal_id: &str) {
        mark_daemon(terminal_id, |probe, now| {
            if probe.pty_write_start_us.is_some() {
                probe.pty_output_us.get_or_insert(now);
            }
        });
    }

    pub fn daemon_stream_queued(terminal_id: &str) {
        mark_daemon(terminal_id, |probe, now| {
            if probe.pty_output_us.is_some() {
                probe.stream_us.get_or_insert(now);
            }
        });
        log_completed_daemon(terminal_id);
    }

    fn log_completed_daemon(terminal_id: &str) {
        if !enabled() {
            return;
        }
        let completed = lock_book()
            .map(|mut book| book.daemon_streamed(terminal_id))
            .unwrap_or_default();
        for probe in completed {
            log::info!(
                target: "okena::terminal_latency",
                "terminal_latency_daemon terminal={terminal_id} sample={} input_len={} input_hash={} ws_receive_us={} bridge_us={} pty_queue_us={} pty_write_start_us={} pty_write_end_us={} pty_output_us={} stream_us={}",
                probe.sample,
                probe.input_len,
                probe.input_hash,
                probe.ws_receive_us,
                value(probe.bridge_us),
                value(probe.pty_queue_us),
                value(probe.pty_write_start_us),
                value(probe.pty_write_end_us),
                value(probe.pty_output_us),
                value(probe.stream_us),
            );
        }
    }

    fn mark_client(terminal_id: &str, update: impl Fn(&mut ClientProbe, u64)) {
        with_book(|book| book.mark_client(terminal_id, now_us(), update));
    }

    fn mark_daemon(terminal_id: &str, update: impl Fn(&mut DaemonProbe, u64)) {
        with_book(|book| book.mark_daemon(terminal_id, now_us(), update));
    }

    fn with_book(update: impl FnOnce(&mut ProbeBook)) {
        if !enabled() {
            return;
        }
        if let Some(mut book) = lock_book() {
            update(&mut book);
        }
    }

    fn lock_book() -> Option<std::sync::MutexGuard<'static, ProbeBook>> {
        PROBES
            .get_or_init(|| Mutex::new(ProbeBook::default()))
            .lock()
            .ok()
    }

    fn now_us() -> u64 {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        u64::try_from(micros).unwrap_or(u64::MAX)
    }

    fn next_sample(counters: &mut HashMap<String, u64>, terminal_id: &str) -> u64 {
        let next = counters.entry(terminal_id.to_string()).or_insert(1);
        let sample = *next;
        *next = next.saturating_add(1);
        sample
    }

    fn trim_pending<T>(pending: &mut VecDeque<T>) {
        if pending.len() >= MAX_PENDING_PER_TERMINAL {
            pending.pop_front();
        }
    }

    fn input_hash(data: &[u8]) -> u64 {
        data.iter().fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
    }

    fn value(timestamp: Option<u64>) -> u64 {
        timestamp.unwrap_or(0)
    }

    #[cfg(test)]
    mod tests {
        use super::ProbeBook;

        #[test]
        fn client_probe_completes_only_in_originating_viewer() {
            let mut book = ProbeBook::default();
            let sample = book.start_client("remote:local:t1", 7, b"x", 100);
            book.mark_client("remote:local:t1", 120, |probe, now| {
                probe.output_receive_us = Some(now);
            });
            book.mark_client("remote:local:t1", 125, |probe, now| {
                probe.parsed_us = Some(now);
            });

            assert!(book.client_painted("remote:local:t1", 8, 160).is_empty());
            assert_eq!(book.client_painted("remote:local:t1", 7, 160), vec![sample]);
            assert!(
                book.client_framed("remote:local:t1", 8, &[sample])
                    .is_empty()
            );
            let completed = book.client_framed("remote:local:t1", 7, &[sample]);
            assert_eq!(completed.len(), 1);
            assert_eq!(completed[0].input_us, 100);
            assert_eq!(completed[0].paint_us, Some(160));
        }

        #[test]
        fn daemon_probe_waits_for_pty_output_before_stream_completion() {
            let mut book = ProbeBook::default();
            let sample = book.start_daemon("t1", b"x", 200);

            assert!(book.daemon_streamed("t1").is_empty());
            book.mark_daemon("t1", 230, |probe, now| {
                probe.pty_output_us = Some(now);
                probe.pty_write_end_us = Some(now - 1);
                probe.stream_us = Some(now + 1);
            });
            let completed = book.daemon_streamed("t1");
            assert_eq!(completed.len(), 1);
            assert_eq!(completed[0].sample, sample);
            assert_eq!(completed[0].pty_output_us, Some(230));
        }

        #[test]
        fn input_hash_and_sample_order_match_for_repeated_bytes() {
            let mut book = ProbeBook::default();
            let first = book.start_client("t1", 1, b"a", 1);
            let second = book.start_client("t1", 1, b"a", 2);
            let pending = book.client.get("t1").expect("client probes");

            assert_eq!((first, second), (1, 2));
            assert_eq!(pending[0].input_hash, pending[1].input_hash);
        }
    }
}

#[cfg(not(feature = "terminal-latency-probe"))]
mod imp {
    #[inline(always)]
    pub fn enabled() -> bool {
        false
    }

    #[inline(always)]
    pub fn client_start(_terminal_id: &str, _viewer: u64, _data: &[u8]) {}

    #[inline(always)]
    pub fn client_output_received(_terminal_id: &str) {}

    #[inline(always)]
    pub fn client_output_parsed(_terminal_id: &str) {}

    #[inline(always)]
    pub fn client_activity_emitted(_terminal_id: &str) {}

    #[inline(always)]
    pub fn client_activity_received(_terminal_id: &str) {}

    #[inline(always)]
    pub fn client_repaint_dispatched(_terminal_id: &str) {}

    #[inline(always)]
    pub fn client_notify_requested(_terminal_id: &str, _viewer: u64) {}

    #[inline(always)]
    pub fn client_painted(_terminal_id: &str, _viewer: u64) -> Vec<u64> {
        Vec::new()
    }

    #[inline(always)]
    pub fn client_frame_completed(_terminal_id: &str, _viewer: u64, _samples: &[u64]) {}

    #[inline(always)]
    pub fn daemon_input_received(_terminal_id: &str, _data: &[u8]) {}

    #[inline(always)]
    pub fn daemon_bridge_received(_terminal_id: &str) {}

    #[inline(always)]
    pub fn daemon_pty_queued(_terminal_id: &str) {}

    #[inline(always)]
    pub fn daemon_pty_write_started(_terminal_id: &str) {}

    #[inline(always)]
    pub fn daemon_pty_write_completed(_terminal_id: &str) {}

    #[inline(always)]
    pub fn daemon_pty_output_received(_terminal_id: &str) {}

    #[inline(always)]
    pub fn daemon_stream_queued(_terminal_id: &str) {}
}

pub use imp::*;
