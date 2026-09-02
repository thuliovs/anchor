pub mod udp;

use crate::protocol::MotionSampleV1;
use serde::Serialize;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const DEFAULT_RECEIVER_HOST: &str = "0.0.0.0";
pub const DEFAULT_RECEIVER_PORT: u16 = 57_421;
pub const EXPECTED_SAMPLE_RATE_HZ: u16 = 60;
pub const MAX_DATAGRAMS_PER_SECOND: u32 = 240;
pub const STALE_AFTER: Duration = Duration::from_millis(250);
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(1);

pub type SharedReceiverState = Arc<Mutex<ReceiverState>>;

#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedSampleEvent {
    pub sample: MotionSampleV1,
    pub sender: SocketAddr,
    pub received_at: Instant,
}

pub trait AcceptedSampleSink: Send + Sync {
    fn try_publish(&self, event: AcceptedSampleEvent);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    Active,
    Stale,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiverStatusDto {
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverSnapshotDto {
    pub status: ReceiverStatusDto,
    pub last_sample: Option<MotionSampleV1>,
    pub active_sender: Option<String>,
    pub active_session_id: Option<String>,
    pub last_sequence: Option<u32>,
    pub last_valid_age_ms: Option<u64>,
    pub metrics: ReceiverMetricsDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverMetricsDto {
    pub received_datagrams: u64,
    pub accepted_samples: u64,
    pub oversized_datagrams: u64,
    pub invalid_packets: u64,
    pub duplicate_or_out_of_order_packets: u64,
    pub foreign_session_packets: u64,
    pub rate_limited_datagrams: u64,
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

impl ReceiverSnapshot {
    pub fn to_dto(&self, now: Instant) -> ReceiverSnapshotDto {
        ReceiverSnapshotDto {
            status: self.status.into(),
            last_sample: self.last_sample.clone(),
            active_sender: self.active_sender.map(|sender| sender.to_string()),
            active_session_id: self.active_session_id.clone(),
            last_sequence: self.last_sequence,
            last_valid_age_ms: self.last_valid_received_at.map(|last_seen| {
                let elapsed_ms = now.saturating_duration_since(last_seen).as_millis();
                u64::try_from(elapsed_ms).unwrap_or(u64::MAX)
            }),
            metrics: self.metrics.clone().into(),
        }
    }
}

impl From<ReceiverMetrics> for ReceiverMetricsDto {
    fn from(value: ReceiverMetrics) -> Self {
        Self {
            received_datagrams: value.received_datagrams,
            accepted_samples: value.accepted_samples,
            oversized_datagrams: value.oversized_datagrams,
            invalid_packets: value.invalid_packets,
            duplicate_or_out_of_order_packets: value.duplicate_or_out_of_order_packets,
            foreign_session_packets: value.foreign_session_packets,
            rate_limited_datagrams: value.rate_limited_datagrams,
        }
    }
}

impl From<StreamStatus> for ReceiverStatusDto {
    fn from(value: StreamStatus) -> Self {
        match value {
            StreamStatus::Active => Self::Active,
            StreamStatus::Stale => Self::Stale,
            StreamStatus::Disconnected => Self::Disconnected,
        }
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
        FixedWindowRateLimiter, ReceiverMetrics, ReceiverState, ReceiverStatusDto,
        SharedReceiverState, StreamStatus, MAX_DATAGRAMS_PER_SECOND, SESSION_TIMEOUT, STALE_AFTER,
    };
    use crate::protocol::MotionSampleV1;
    use serde_json::json;
    use std::{
        net::SocketAddr,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use tokio::{net::UdpSocket, sync::watch};

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
    fn snapshot_dto_serializes_with_camel_case() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(12, "session-a"), sender(41000), now);

        let serialized = serde_json::to_value(state.snapshot(now).to_dto(now)).expect("serialize");

        assert_eq!(serialized["status"], json!("active"));
        assert_eq!(serialized["activeSender"], json!("127.0.0.1:41000"));
        assert_eq!(serialized["activeSessionId"], json!("session-a"));
        assert_eq!(serialized["lastSequence"], json!(12));
        assert!(serialized.get("active_sender").is_none());
        assert!(serialized["metrics"].get("receivedDatagrams").is_some());
    }

    #[test]
    fn snapshot_dto_preserves_status_values() {
        assert_eq!(
            ReceiverStatusDto::from(StreamStatus::Active),
            ReceiverStatusDto::Active
        );
        assert_eq!(
            ReceiverStatusDto::from(StreamStatus::Stale),
            ReceiverStatusDto::Stale
        );
        assert_eq!(
            ReceiverStatusDto::from(StreamStatus::Disconnected),
            ReceiverStatusDto::Disconnected
        );
    }

    #[test]
    fn snapshot_dto_calculates_monotonic_age() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(0, "session-a"), sender(41000), now);

        let dto = state.snapshot(now).to_dto(now + Duration::from_millis(333));

        assert_eq!(dto.last_valid_age_ms, Some(333));
    }

    #[test]
    fn snapshot_dto_converts_sender_address_to_string() {
        let now = Instant::now();
        let mut state = ReceiverState::default();
        state.apply_sample(sample(0, "session-a"), sender(41042), now);

        let dto = state.snapshot(now).to_dto(now);

        assert_eq!(dto.active_sender.as_deref(), Some("127.0.0.1:41042"));
    }

    #[test]
    fn snapshot_dto_preserves_metrics() {
        let snapshot = super::ReceiverSnapshot {
            last_sample: None,
            active_sender: None,
            active_session_id: None,
            last_sequence: None,
            last_valid_received_at: None,
            status: StreamStatus::Disconnected,
            metrics: ReceiverMetrics {
                received_datagrams: 10,
                accepted_samples: 8,
                oversized_datagrams: 1,
                invalid_packets: 2,
                duplicate_or_out_of_order_packets: 3,
                foreign_session_packets: 4,
                rate_limited_datagrams: 5,
            },
        };

        let dto = snapshot.to_dto(Instant::now());

        assert_eq!(dto.metrics.received_datagrams, 10);
        assert_eq!(dto.metrics.accepted_samples, 8);
        assert_eq!(dto.metrics.oversized_datagrams, 1);
        assert_eq!(dto.metrics.invalid_packets, 2);
        assert_eq!(dto.metrics.duplicate_or_out_of_order_packets, 3);
        assert_eq!(dto.metrics.foreign_session_packets, 4);
        assert_eq!(dto.metrics.rate_limited_datagrams, 5);
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

    #[test]
    fn udp_receiver_default_bind_addr_listens_on_all_ipv4_interfaces() {
        let config = UdpReceiverConfig::default();

        assert_eq!(
            config.bind_addr,
            "0.0.0.0:57421".parse().expect("bind addr")
        );
    }

    #[tokio::test]
    async fn udp_receiver_accepts_valid_datagram_and_updates_state() {
        let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
        let (sample_tx, mut sample_rx) = watch::channel(None);
        let receiver = start_udp_receiver(
            UdpReceiverConfig {
                bind_addr: "127.0.0.1:0".parse().expect("bind addr"),
                ..UdpReceiverConfig::default()
            },
            shared_state.clone(),
            sample_tx,
        )
        .await
        .expect("receiver should start");

        let sender_socket = UdpSocket::bind("127.0.0.1:0").await.expect("sender bind");
        sender_socket
            .send_to(VALID_FIXTURE.as_bytes(), receiver.local_addr())
            .await
            .expect("send datagram");

        sample_rx.changed().await.expect("watch update");
        let published_sample = sample_rx
            .borrow_and_update()
            .clone()
            .expect("accepted sample should be published");

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
        assert_eq!(
            published_sample.sequence,
            snapshot.last_sequence.expect("sequence")
        );

        receiver.shutdown().await;
    }

    #[tokio::test]
    async fn udp_receiver_does_not_publish_ignored_samples() {
        let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
        let (sample_tx, mut sample_rx) = watch::channel(None);
        let receiver = start_udp_receiver(
            UdpReceiverConfig {
                bind_addr: "127.0.0.1:0".parse().expect("bind addr"),
                ..UdpReceiverConfig::default()
            },
            shared_state.clone(),
            sample_tx,
        )
        .await
        .expect("receiver should start");

        let sender_socket = UdpSocket::bind("127.0.0.1:0").await.expect("sender bind");
        sender_socket
            .send_to(VALID_FIXTURE.as_bytes(), receiver.local_addr())
            .await
            .expect("send initial datagram");

        sample_rx.changed().await.expect("first watch update");
        let _ = sample_rx.borrow_and_update().clone();

        sender_socket
            .send_to(VALID_FIXTURE.as_bytes(), receiver.local_addr())
            .await
            .expect("send duplicate datagram");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let duplicate_count = {
                let state = shared_state.lock().expect("shared state lock");
                state
                    .snapshot(Instant::now())
                    .metrics
                    .duplicate_or_out_of_order_packets
            };

            if duplicate_count == 1 {
                break;
            }

            assert!(
                Instant::now() < deadline,
                "receiver did not process duplicate datagram in time"
            );
            tokio::task::yield_now().await;
        }

        assert!(
            !sample_rx.has_changed().expect("watch receiver state"),
            "ignored samples must not be published"
        );

        receiver.shutdown().await;
    }

    #[tokio::test]
    async fn receiver_watch_channel_coalesces_intermediate_samples() {
        let (sample_tx, mut sample_rx) = watch::channel(None);

        sample_tx
            .send(Some(sample(1, "session-a")))
            .expect("send sample 1");
        sample_tx
            .send(Some(sample(2, "session-a")))
            .expect("send sample 2");
        sample_tx
            .send(Some(sample(3, "session-a")))
            .expect("send sample 3");

        sample_rx.changed().await.expect("watch update");
        let latest = sample_rx
            .borrow_and_update()
            .clone()
            .expect("latest sample should be available");

        assert_eq!(latest.sequence, 3);
        assert!(
            !sample_rx.has_changed().expect("watch receiver state"),
            "watch should not queue every intermediate sample"
        );
    }
}
