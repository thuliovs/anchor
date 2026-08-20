pub mod udp;

use crate::protocol::MotionSampleV1;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const DEFAULT_RECEIVER_HOST: &str = "127.0.0.1";
pub const DEFAULT_RECEIVER_PORT: u16 = 57_421;
pub const EXPECTED_SAMPLE_RATE_HZ: u16 = 60;
pub const MAX_DATAGRAMS_PER_SECOND: u32 = 240;
pub const STALE_AFTER: Duration = Duration::from_millis(250);
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(1);

pub type SharedReceiverState = Arc<Mutex<ReceiverState>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    Active,
    Stale,
    Disconnected,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiverMetrics {
    pub received_datagrams: u64,
    pub accepted_samples: u64,
    pub oversized_datagrams: u64,
    pub invalid_packets: u64,
    pub duplicate_or_out_of_order_packets: u64,
    pub foreign_session_packets: u64,
    pub rate_limited_datagrams: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiverSnapshot {
    pub last_sample: Option<MotionSampleV1>,
    pub active_sender: Option<SocketAddr>,
    pub active_session_id: Option<String>,
    pub last_sequence: Option<u32>,
    pub last_valid_received_at: Option<Instant>,
    pub metrics: ReceiverMetrics,
    pub status: StreamStatus,
}

#[derive(Debug, Default)]
pub struct ReceiverState {
    last_sample: Option<MotionSampleV1>,
    active_sender: Option<SocketAddr>,
    active_session_id: Option<String>,
    last_sequence: Option<u32>,
    last_valid_received_at: Option<Instant>,
    metrics: ReceiverMetrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleAcceptance {
    AcceptedNewSession,
    AcceptedSessionTakeover,
    AcceptedInOrder,
    IgnoredDuplicateOrOutOfOrder,
    IgnoredDifferentSession,
}

impl SampleAcceptance {
    pub fn was_accepted(self) -> bool {
        matches!(
            self,
            Self::AcceptedNewSession | Self::AcceptedSessionTakeover | Self::AcceptedInOrder
        )
    }
}

impl ReceiverState {
    pub fn record_received_datagram(&mut self) {
        self.metrics.received_datagrams += 1;
    }

    pub fn record_oversized_datagram(&mut self) {
        self.metrics.oversized_datagrams += 1;
    }

    pub fn record_invalid_packet(&mut self) {
        self.metrics.invalid_packets += 1;
    }

    pub fn record_rate_limited_datagram(&mut self) {
        self.metrics.rate_limited_datagrams += 1;
    }

    pub fn apply_sample(
        &mut self,
        sample: MotionSampleV1,
        sender: SocketAddr,
        now: Instant,
    ) -> SampleAcceptance {
        match self.active_session_id.as_deref() {
            None => self.accept_sample(sample, sender, now, SampleAcceptance::AcceptedNewSession),
            Some(active_session_id) if active_session_id == sample.session_id => {
                if let Some(last_sequence) = self.last_sequence {
                    // MVP: sequence rollover is intentionally unsupported for v1.
                    if sample.sequence <= last_sequence {
                        self.metrics.duplicate_or_out_of_order_packets += 1;
                        return SampleAcceptance::IgnoredDuplicateOrOutOfOrder;
                    }
                }

                self.accept_sample(sample, sender, now, SampleAcceptance::AcceptedInOrder)
            }
            Some(_) if self.is_session_timed_out(now) => self.accept_sample(
                sample,
                sender,
                now,
                SampleAcceptance::AcceptedSessionTakeover,
            ),
            Some(_) => {
                self.metrics.foreign_session_packets += 1;
                SampleAcceptance::IgnoredDifferentSession
            }
        }
    }

    pub fn snapshot(&self, now: Instant) -> ReceiverSnapshot {
        ReceiverSnapshot {
            last_sample: self.last_sample.clone(),
            active_sender: self.active_sender,
            active_session_id: self.active_session_id.clone(),
            last_sequence: self.last_sequence,
            last_valid_received_at: self.last_valid_received_at,
            metrics: self.metrics.clone(),
            status: self.stream_status(now),
        }
    }

    pub fn stream_status(&self, now: Instant) -> StreamStatus {
        match self.last_valid_received_at {
            None => StreamStatus::Disconnected,
            Some(last_seen) => {
                let elapsed = now.saturating_duration_since(last_seen);
                if elapsed > SESSION_TIMEOUT {
                    StreamStatus::Disconnected
                } else if elapsed > STALE_AFTER {
                    StreamStatus::Stale
                } else {
                    StreamStatus::Active
                }
            }
        }
    }

    fn is_session_timed_out(&self, now: Instant) -> bool {
        matches!(self.stream_status(now), StreamStatus::Disconnected)
    }

    fn accept_sample(
        &mut self,
        sample: MotionSampleV1,
        sender: SocketAddr,
        now: Instant,
        acceptance: SampleAcceptance,
    ) -> SampleAcceptance {
        self.active_sender = Some(sender);
        self.last_sequence = Some(sample.sequence);
        self.active_session_id = Some(sample.session_id.clone());
        self.last_valid_received_at = Some(now);
        self.last_sample = Some(sample);
        self.metrics.accepted_samples += 1;
        acceptance
    }
}

#[derive(Debug, Clone)]
pub struct FixedWindowRateLimiter {
    limit: u32,
    window: Duration,
    window_started_at: Option<Instant>,
    seen_in_window: u32,
}

impl FixedWindowRateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            window_started_at: None,
            seen_in_window: 0,
        }
    }

    pub fn allow(&mut self, now: Instant) -> bool {
        match self.window_started_at {
            Some(window_started_at)
                if now.saturating_duration_since(window_started_at) < self.window => {}
            _ => {
                self.window_started_at = Some(now);
                self.seen_in_window = 0;
            }
        }

        if self.seen_in_window < self.limit {
            self.seen_in_window += 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        udp::{start_udp_receiver, UdpReceiverConfig},
        FixedWindowRateLimiter, ReceiverState, SharedReceiverState, StreamStatus,
        MAX_DATAGRAMS_PER_SECOND, SESSION_TIMEOUT, STALE_AFTER,
    };
    use crate::protocol::MotionSampleV1;
    use std::{
        net::SocketAddr,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use tokio::net::UdpSocket;

    const VALID_FIXTURE: &str =
        include_str!("../../../../../packages/protocol/fixtures/valid/motion-sample.json");

    fn sample(sequence: u32, session_id: &str) -> MotionSampleV1 {
        let mut sample: MotionSampleV1 =
            serde_json::from_str(VALID_FIXTURE).expect("valid fixture");
        sample.sequence = sequence;
        sample.session_id = session_id.to_owned();
        sample
    }

    fn sender(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().expect("sender addr")
    }

    #[test]
    fn first_valid_session_assumes_state() {
        let now = Instant::now();
        let mut state = ReceiverState::default();

        let accepted = state.apply_sample(sample(0, "session-a"), sender(41000), now);
        let snapshot = state.snapshot(now);

        assert!(accepted.was_accepted());
        assert_eq!(snapshot.active_session_id.as_deref(), Some("session-a"));
        assert_eq!(snapshot.last_sequence, Some(0));
        assert_eq!(snapshot.active_sender, Some(sender(41000)));
        assert_eq!(snapshot.status, StreamStatus::Active);
    }

    #[test]
    fn increasing_sequence_is_accepted() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(0, "session-a"), sender(41000), now);

        let accepted = state.apply_sample(
            sample(1, "session-a"),
            sender(41000),
            now + Duration::from_millis(10),
        );
        let snapshot = state.snapshot(now + Duration::from_millis(10));

        assert!(accepted.was_accepted());
        assert_eq!(snapshot.last_sequence, Some(1));
        assert_eq!(snapshot.metrics.accepted_samples, 2);
    }

    #[test]
    fn repeated_sequence_is_ignored() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(3, "session-a"), sender(41000), now);

        let accepted = state.apply_sample(
            sample(3, "session-a"),
            sender(41000),
            now + Duration::from_millis(1),
        );
        let snapshot = state.snapshot(now + Duration::from_millis(1));

        assert!(!accepted.was_accepted());
        assert_eq!(snapshot.last_sequence, Some(3));
        assert_eq!(snapshot.metrics.duplicate_or_out_of_order_packets, 1);
    }

    #[test]
    fn lower_sequence_is_ignored() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(7, "session-a"), sender(41000), now);

        let accepted = state.apply_sample(
            sample(6, "session-a"),
            sender(41000),
            now + Duration::from_millis(1),
        );
        let snapshot = state.snapshot(now + Duration::from_millis(1));

        assert!(!accepted.was_accepted());
        assert_eq!(snapshot.last_sequence, Some(7));
        assert_eq!(snapshot.metrics.duplicate_or_out_of_order_packets, 1);
    }

    #[test]
    fn different_session_is_ignored_before_timeout() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(0, "session-a"), sender(41000), now);

        let accepted = state.apply_sample(
            sample(0, "session-b"),
            sender(41001),
            now + Duration::from_millis(500),
        );
        let snapshot = state.snapshot(now + Duration::from_millis(500));

        assert!(!accepted.was_accepted());
        assert_eq!(snapshot.active_session_id.as_deref(), Some("session-a"));
        assert_eq!(snapshot.metrics.foreign_session_packets, 1);
    }

    #[test]
    fn different_session_can_take_over_after_timeout() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(0, "session-a"), sender(41000), now);

        let accepted = state.apply_sample(
            sample(0, "session-b"),
            sender(41001),
            now + SESSION_TIMEOUT + Duration::from_millis(1),
        );
        let snapshot = state.snapshot(now + SESSION_TIMEOUT + Duration::from_millis(1));

        assert!(accepted.was_accepted());
        assert_eq!(snapshot.active_session_id.as_deref(), Some("session-b"));
        assert_eq!(snapshot.active_sender, Some(sender(41001)));
    }

    #[test]
    fn status_changes_from_active_to_stale() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(0, "session-a"), sender(41000), now);

        let snapshot = state.snapshot(now + STALE_AFTER + Duration::from_millis(1));

        assert_eq!(snapshot.status, StreamStatus::Stale);
    }

    #[test]
    fn status_changes_from_stale_to_disconnected() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(0, "session-a"), sender(41000), now);

        let stale = state.snapshot(now + STALE_AFTER + Duration::from_millis(1));
        let disconnected = state.snapshot(now + SESSION_TIMEOUT + Duration::from_millis(1));

        assert_eq!(stale.status, StreamStatus::Stale);
        assert_eq!(disconnected.status, StreamStatus::Disconnected);
        assert!(disconnected.last_sample.is_some());
    }

    #[test]
    fn metrics_are_updated_correctly() {
        let now = Instant::now();
        let mut state = ReceiverState::default();

        state.record_received_datagram();
        state.apply_sample(sample(0, "session-a"), sender(41000), now);
        state.record_received_datagram();
        state.apply_sample(
            sample(0, "session-a"),
            sender(41000),
            now + Duration::from_millis(1),
        );
        state.record_received_datagram();
        state.apply_sample(
            sample(0, "session-b"),
            sender(41001),
            now + Duration::from_millis(2),
        );
        state.record_invalid_packet();
        state.record_oversized_datagram();
        state.record_rate_limited_datagram();

        let snapshot = state.snapshot(now + Duration::from_millis(2));

        assert_eq!(snapshot.metrics.received_datagrams, 3);
        assert_eq!(snapshot.metrics.accepted_samples, 1);
        assert_eq!(snapshot.metrics.duplicate_or_out_of_order_packets, 1);
        assert_eq!(snapshot.metrics.foreign_session_packets, 1);
        assert_eq!(snapshot.metrics.invalid_packets, 1);
        assert_eq!(snapshot.metrics.oversized_datagrams, 1);
        assert_eq!(snapshot.metrics.rate_limited_datagrams, 1);
    }

    #[test]
    fn rate_limiter_allows_up_to_limit_per_window() {
        let start = Instant::now();
        let mut limiter =
            FixedWindowRateLimiter::new(MAX_DATAGRAMS_PER_SECOND, Duration::from_secs(1));

        for index in 0..MAX_DATAGRAMS_PER_SECOND {
            assert!(limiter.allow(start + Duration::from_millis(u64::from(index))));
        }
    }

    #[test]
    fn rate_limiter_rejects_excess_packets_and_resets_next_window() {
        let start = Instant::now();
        let mut limiter =
            FixedWindowRateLimiter::new(MAX_DATAGRAMS_PER_SECOND, Duration::from_secs(1));

        for _ in 0..MAX_DATAGRAMS_PER_SECOND {
            assert!(limiter.allow(start));
        }

        assert!(!limiter.allow(start + Duration::from_millis(999)));
        assert!(limiter.allow(start + Duration::from_secs(1) + Duration::from_millis(1)));
    }

    #[tokio::test]
    async fn udp_receiver_accepts_valid_datagram_and_updates_state() {
        let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
        let receiver = start_udp_receiver(
            UdpReceiverConfig {
                bind_addr: "127.0.0.1:0".parse().expect("bind addr"),
                ..UdpReceiverConfig::default()
            },
            shared_state.clone(),
        )
        .await
        .expect("receiver should start");

        let sender_socket = UdpSocket::bind("127.0.0.1:0").await.expect("sender bind");
        sender_socket
            .send_to(VALID_FIXTURE.as_bytes(), receiver.local_addr())
            .await
            .expect("send datagram");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let accepted = {
                let state = shared_state.lock().expect("shared state lock");
                state.snapshot(Instant::now()).metrics.accepted_samples
            };
            if accepted == 1 {
                break;
            }

            assert!(
                Instant::now() < deadline,
                "receiver did not accept the datagram in time"
            );
            tokio::task::yield_now().await;
        }

        let snapshot = shared_state
            .lock()
            .expect("shared state lock")
            .snapshot(Instant::now());
        assert_eq!(
            snapshot.active_session_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(snapshot.metrics.accepted_samples, 1);

        receiver.shutdown().await;
    }
}
