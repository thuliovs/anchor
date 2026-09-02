use super::{
    AcceptedSampleEvent, AcceptedSampleSink, FixedWindowRateLimiter, SampleAcceptance,
    SharedReceiverState, DEFAULT_RECEIVER_PORT, EXPECTED_SAMPLE_RATE_HZ, MAX_DATAGRAMS_PER_SECOND,
};
use crate::protocol::{parse_validated_datagram, ProtocolParseError, MAX_DATAGRAM_BYTES};
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::UdpSocket,
    sync::{oneshot, watch},
    task::JoinHandle,
    time::{interval_at, Instant as TokioInstant, MissedTickBehavior},
};

#[derive(Clone)]
pub struct UdpReceiverConfig {
    pub bind_addr: SocketAddr,
    pub max_datagrams_per_second: u32,
    pub metrics_log_interval: Duration,
    pub accepted_sample_sink: Option<Arc<dyn AcceptedSampleSink>>,
}

impl Default for UdpReceiverConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_RECEIVER_PORT)),
            max_datagrams_per_second: MAX_DATAGRAMS_PER_SECOND,
            metrics_log_interval: Duration::from_secs(1),
            accepted_sample_sink: None,
        }
    }
}

#[derive(Debug)]
pub struct UdpReceiverHandle {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
}

impl UdpReceiverHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Err(err) = self.join_handle.await {
            eprintln!("receiver shutdown join error: {err}");
        }
    }
}

pub async fn start_udp_receiver(
    config: UdpReceiverConfig,
    shared_state: SharedReceiverState,
    latest_sample_tx: watch::Sender<Option<crate::protocol::MotionSampleV1>>,
) -> io::Result<UdpReceiverHandle> {
    let socket = UdpSocket::bind(config.bind_addr).await?;
    let local_addr = socket.local_addr()?;
    println!(
        "motion receiver listening on {local_addr} across all IPv4 interfaces (expected sample rate: {}Hz)",
        EXPECTED_SAMPLE_RATE_HZ
    );

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let accepted_sample_sink = config.accepted_sample_sink.clone();
    let join_handle = tokio::spawn(async move {
        let mut limiter =
            FixedWindowRateLimiter::new(config.max_datagrams_per_second, Duration::from_secs(1));
        let mut metrics_tick = interval_at(
            TokioInstant::now() + config.metrics_log_interval,
            config.metrics_log_interval,
        );
        metrics_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut buffer = [0_u8; MAX_DATAGRAM_BYTES + 1];

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    println!("motion receiver stopping on {local_addr}");
                    break;
                }
                _ = metrics_tick.tick() => {
                    if let Ok(state) = shared_state.lock() {
                        let snapshot = state.snapshot(std::time::Instant::now());
                        println!(
                            "motion receiver metrics: received={} accepted={} oversized={} invalid={} duplicate_or_out_of_order={} foreign_session={} rate_limited={} status={:?} bind_scope=all_ipv4_interfaces",
                            snapshot.metrics.received_datagrams,
                            snapshot.metrics.accepted_samples,
                            snapshot.metrics.oversized_datagrams,
                            snapshot.metrics.invalid_packets,
                            snapshot.metrics.duplicate_or_out_of_order_packets,
                            snapshot.metrics.foreign_session_packets,
                            snapshot.metrics.rate_limited_datagrams,
                            snapshot.status,
                        );
                    } else {
                        eprintln!("motion receiver state lock poisoned during metrics logging");
                    }
                }
                recv_result = socket.recv_from(&mut buffer) => {
                    let now = std::time::Instant::now();

                    match recv_result {
                        Ok((received_len, sender_addr)) => {
                            let outcome = if let Ok(mut state) = shared_state.lock() {
                                state.record_received_datagram();

                                if !limiter.allow(now) {
                                    state.record_rate_limited_datagram();
                                    None
                                } else if received_len > MAX_DATAGRAM_BYTES {
                                    state.record_oversized_datagram();
                                    None
                                } else {
                                    match parse_validated_datagram(&buffer[..received_len]) {
                                        Ok(sample) => {
                                            let published_sample = sample.clone();
                                            let acceptance = state.apply_sample(sample, sender_addr, now);
                                            Some((acceptance, acceptance.was_accepted().then_some(published_sample)))
                                        }
                                        Err(ProtocolParseError::DatagramTooLarge { .. }) => {
                                            state.record_oversized_datagram();
                                            None
                                        }
                                        Err(_) => {
                                            state.record_invalid_packet();
                                            None
                                        }
                                    }
                                }
                            } else {
                                eprintln!("motion receiver state lock poisoned while processing datagram");
                                None
                            };

                            if let Some((acceptance, published_sample)) = outcome {
                                if let Some(sample) = published_sample {
                                    if let Some(sink) = accepted_sample_sink.as_ref() {
                                        sink.try_publish(AcceptedSampleEvent {
                                            sample: sample.clone(),
                                            sender: sender_addr,
                                            received_at: now,
                                        });
                                    }
                                    let _ = latest_sample_tx.send(Some(sample));
                                }

                                match acceptance {
                                    SampleAcceptance::AcceptedNewSession => {
                                        println!("motion receiver session started from {sender_addr}");
                                    }
                                    SampleAcceptance::AcceptedSessionTakeover => {
                                        println!("motion receiver session changed after timeout from {sender_addr}");
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(err) => {
                            eprintln!("motion receiver socket error on {local_addr}: {err}");
                        }
                    }
                }
            }
        }
    });

    Ok(UdpReceiverHandle {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}
