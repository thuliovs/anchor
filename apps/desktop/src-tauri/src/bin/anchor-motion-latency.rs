use anchor_desktop_lib::{
    latency_measurement::{
        clock_anchor, load_latency_dataset, parse_packet, report_from_analysis, B4bConfigurationV1,
        ClockAnchorV1, Counters, LatencyPacketV1, RawSampleV1, LATENCY_DATASET_VERSION,
        MAX_LATENCY_DATAGRAM_BYTES,
    },
    latency_recorder::{LatencyRecord, LatencyRecorder, LATENCY_RECORDER_CAPACITY},
    latency_session::{InterruptDecision, LatencySessionLifecycle, LifecycleDecision},
    live_motion::{runtime::load_pipeline_from_paths, LiveMotionState},
    motion_filtering::selection::TiltEstimatorPolicyV2,
    receiver::{
        AcceptedSampleEvent, FixedWindowRateLimiter, ReceiverState, SampleAcceptance,
        MAX_DATAGRAMS_PER_SECOND,
    },
    transport_measurement::Distribution,
};
use rand::RngCore;
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct RuntimeWork {
    event: AcceptedSampleEvent,
    raw: RawSampleV1,
}

fn join_runtime_worker(worker: thread::JoinHandle<()>) -> Result<(), String> {
    worker
        .join()
        .map_err(|_| "B4b runtime worker panicked".to_owned())
}

#[derive(Debug)]
struct Config {
    duration: u64,
    timeout: u64,
    final_sync_timeout: u64,
    bind: SocketAddr,
    profile: PathBuf,
    policy: PathBuf,
    dataset: PathBuf,
    report: PathBuf,
}
fn parse(a: &[String]) -> Result<Config, String> {
    let mut duration = 60;
    let mut timeout = 60;
    let mut final_sync_timeout = 5;
    let mut bind = "0.0.0.0:57421".parse().unwrap();
    let mut profile = None;
    let mut policy = None;
    let mut dataset = None;
    let mut report_path = None;
    let mut i = 0;
    while i < a.len() {
        if a[i] == "--" {
            i += 1;
            continue;
        }
        let f = &a[i];
        let v = a
            .get(i + 1)
            .ok_or_else(|| format!("missing value for {f}"))?;
        match f.as_str() {
            "--duration-seconds" => duration = positive(v, f)?,
            "--startup-timeout-seconds" => timeout = positive(v, f)?,
            "--final-sync-timeout-seconds" => final_sync_timeout = positive(v, f)?,
            "--bind" => bind = v.parse().map_err(|e| format!("invalid bind: {e}"))?,
            "--profile" => profile = once(profile, v, f)?,
            "--policy" => policy = once(policy, v, f)?,
            "--dataset-output" => dataset = once(dataset, v, f)?,
            "--report-output" => report_path = once(report_path, v, f)?,
            _ => return Err(format!("unsupported option {f}")),
        }
        i += 2
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    Ok(Config {
        duration,
        timeout,
        final_sync_timeout,
        bind,
        profile: profile.ok_or("--profile is required")?,
        policy: policy.ok_or("--policy is required")?,
        dataset: dataset.unwrap_or_else(|| {
            format!("artifacts/motion-latency-datasets/{stamp}-b5b-v1.ndjson").into()
        }),
        report: report_path.unwrap_or_else(|| {
            format!("artifacts/motion-latency-measurements/{stamp}-b5b-v1.json").into()
        }),
    })
}
fn once(old: Option<PathBuf>, v: &str, f: &str) -> Result<Option<PathBuf>, String> {
    if old.is_some() {
        Err(format!("{f} may only be provided once"))
    } else {
        Ok(Some(v.into()))
    }
}
fn positive(v: &str, f: &str) -> Result<u64, String> {
    let n = v
        .parse()
        .map_err(|_| format!("{f} must be a positive integer"))?;
    if n == 0 {
        Err(format!("{f} must be a positive integer"))
    } else {
        Ok(n)
    }
}
fn us(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_micros()).unwrap_or(u64::MAX)
}
fn nonce() -> String {
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|v| format!("{v:02x}")).collect()
}
fn percentiles(value: &Distribution) -> String {
    match value {
        Distribution::Available { p50, p95, p99, .. } => {
            format!("p50={p50:.3} p95={p95:.3} p99={p99:.3}")
        }
        Distribution::Unavailable { .. } => "p50=n/a p95=n/a p99=n/a".into(),
    }
}
#[tokio::main]
async fn main() -> ExitCode {
    command_exit(run(parse(&env::args().skip(1).collect::<Vec<_>>())).await)
}
fn command_exit(result: Result<bool, String>) -> ExitCode {
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
#[allow(clippy::possible_missing_else)]
async fn run(config: Result<Config, String>) -> Result<bool, String> {
    let c = config?;
    if c.dataset.exists() || c.report.exists() {
        return Err("output exists; refusing silent overwrite".into());
    }
    let socket = tokio::net::UdpSocket::bind(c.bind)
        .await
        .map_err(|e| format!("bind failed: {e}"))?;
    let pipeline = load_pipeline_from_paths(&c.profile, &c.policy)
        .map_err(|e| format!("B4b configuration failed: {e}"))?;
    let initial = pipeline.snapshot(Instant::now());
    if initial.validity == LiveMotionState::Unavailable {
        return Err(format!(
            "B4b configuration is incompatible: {:?}",
            initial.neutral_reason
        ));
    }
    let policy: TiltEstimatorPolicyV2 = serde_json::from_slice(
        &fs::read(&c.policy).map_err(|e| format!("failed to read B4b policy: {e}"))?,
    )
    .map_err(|e| format!("failed to preserve B4b policy: {e}"))?;
    let b4b_configuration = B4bConfigurationV1 {
        profile_path: c.profile.display().to_string(),
        policy_path: c.policy.display().to_string(),
        calibration_profile_fingerprint: initial
            .calibration_profile_fingerprint
            .clone()
            .ok_or("B4b configuration did not expose its calibration fingerprint")?,
        policy,
    };
    let (runtime_tx, runtime_rx) = mpsc::sync_channel::<RuntimeWork>(120);
    let (processed_tx, mut processed_rx) = tokio::sync::mpsc::channel::<(RawSampleV1, String)>(120);
    let processed_count = Arc::new(AtomicU64::new(0));
    let worker_processed_count = processed_count.clone();
    let result_drops = Arc::new(AtomicU64::new(0));
    let worker_result_drops = result_drops.clone();
    let recorder = LatencyRecorder::start(c.dataset.clone(), LATENCY_RECORDER_CAPACITY)?;
    let recorder_ingress = recorder.ingress();
    let runtime_recorder = recorder_ingress.clone();
    let origin = Instant::now();
    let runtime_origin = origin;
    let runtime_worker = thread::spawn(move || {
        let mut pipeline = pipeline;
        while let Ok(mut work) = runtime_rx.recv() {
            pipeline.process(work.event);
            worker_processed_count.fetch_add(1, Ordering::Relaxed);
            work.raw.desktop_processed_us = Some(us(runtime_origin));
            let state = format!("{:?}", pipeline.snapshot(Instant::now()).validity).to_lowercase();
            match processed_tx.try_send((work.raw, state)) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full((raw, _))) => {
                    worker_result_drops.fetch_add(1, Ordering::Relaxed);
                    runtime_recorder.try_publish(
                        LatencyRecord::Rejection {
                            measurement_protocol_version: 1,
                            measurement_id: Some(raw.measurement_id),
                            desktop_us: us(runtime_origin),
                            category: "result_drop".into(),
                            reason: "bounded_result_queue_full".into(),
                        },
                        true,
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
            }
        }
    });
    if !recorder_ingress.try_publish(
        LatencyRecord::Metadata {
            latency_dataset_version: LATENCY_DATASET_VERSION,
            measurement_protocol_version: 1,
            requested_duration_us: c.duration * 1_000_000,
            b4b_configuration,
        },
        true,
    ) {
        return Err("recorder rejected metadata".into());
    }
    println!(
        "B5b receiver ready on {}; waiting for latency_hello_v1",
        socket.local_addr().map_err(|e| e.to_string())?
    );
    let startup = tokio::time::Instant::now() + Duration::from_secs(c.timeout);
    let mut buf = vec![0; MAX_LATENCY_DATAGRAM_BYTES + 1];
    let mut endpoint: Option<SocketAddr> = None;
    let mut id: Option<String> = None;
    let mut counters = Counters::default();
    let mut validator = anchor_desktop_lib::latency_measurement::ProbeValidator::default();
    let mut anchors = Vec::<ClockAnchorV1>::new();
    let mut samples = Vec::<RawSampleV1>::new();
    let mut receiver = ReceiverState::default();
    let mut limiter = FixedWindowRateLimiter::new(MAX_DATAGRAMS_PER_SECOND, Duration::from_secs(1));
    let mut lifecycle = LatencySessionLifecycle::new(
        c.duration.saturating_mul(1_000_000),
        c.final_sync_timeout.saturating_mul(1_000_000),
    );
    let mut final_probe_after_window = false;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let now_us = us(origin);
        let window_done = lifecycle.window_done(now_us);
        let runtime_drained = processed_count.load(Ordering::Relaxed) + counters.runtime_dropped
            >= counters.accepted_samples;
        match lifecycle.poll(
            now_us,
            final_probe_after_window,
            runtime_drained,
            anchors.last().map(|anchor| anchor.desktop_midpoint_us),
        ) {
            LifecycleDecision::Complete => break,
            LifecycleDecision::FinalSyncTimedOut => {
                println!("final synchronization timeout reached");
                break;
            }
            LifecycleDecision::Continue => {}
        }
        let finish_deadline = lifecycle.final_sync_deadline_us().map_or_else(
            || tokio::time::Instant::now() + Duration::from_secs(86_400),
            |deadline| {
                tokio::time::Instant::now() + Duration::from_micros(deadline.saturating_sub(now_us))
            },
        );
        tokio::select! {
        _=tokio::time::sleep_until(finish_deadline),if lifecycle.final_sync_deadline_us().is_some()=>{},
        Some((raw,state))=processed_rx.recv()=>{recorder_ingress.try_publish(LatencyRecord::SampleProcessed{measurement_protocol_version:1,measurement_id:raw.measurement_id.clone(),session_id:raw.session_id.clone(),sequence:raw.sequence,desktop_processed_us:raw.desktop_processed_us.unwrap_or(0),processing_completed:true,b4b_state:state},true);samples.push(raw)},
        _=tokio::time::sleep_until(startup),if !lifecycle.has_samples()=>{drop(runtime_tx);join_runtime_worker(runtime_worker)?;drop(recorder_ingress);recorder.abort()?;fs::remove_file(&c.dataset).map_err(|e|format!("failed to remove empty dataset: {e}"))?;return Err("startup timeout before the first accepted B5b sample; dataset without samples removed and no report produced".into())},
        signal=tokio::signal::ctrl_c()=>{signal.map_err(|e|e.to_string())?;if lifecycle.interrupt()==InterruptDecision::AbortBeforeFirstSample{drop(runtime_tx);join_runtime_worker(runtime_worker)?;drop(recorder_ingress);recorder.abort()?;fs::remove_file(&c.dataset).map_err(|e|format!("failed to remove empty dataset: {e}"))?;return Err("Ctrl+C before first sample; empty dataset removed and no report produced".into())}break},
        _=tick.tick(),if endpoint.is_some()=>{let n=nonce();let t1=us(origin);let mid=id.clone().unwrap();let ep=endpoint.unwrap();let packet=LatencyPacketV1::ClockSyncRequestV1{measurement_protocol_version:1,measurement_id:mid.clone(),nonce:n.clone(),t1_desktop_us:t1};let bytes=serde_json::to_vec(&packet).map_err(|e|e.to_string())?;socket.send_to(&bytes,ep).await.map_err(|e|e.to_string())?;validator.sent(n.clone(),ep,mid.clone(),t1);counters.probes_sent+=1;if window_done{final_probe_after_window=true}recorder_ingress.try_publish(LatencyRecord::ClockSyncRequestSent{measurement_protocol_version:1,measurement_id:mid,nonce:n,endpoint:ep.to_string(),t1_desktop_us:t1},true);},
        received=socket.recv_from(&mut buf)=>{let(take,from)=received.map_err(|e|e.to_string())?;counters.datagrams_received+=1;receiver.record_received_datagram();let now=Instant::now();let ingress=us(origin);if !limiter.allow(now){counters.rate_limited_packets+=1;receiver.record_rate_limited_datagram();recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:id.clone(),desktop_us:ingress,category:"rate_limited".into(),reason:"fixed_window_limit".into()},false);continue}if take>MAX_LATENCY_DATAGRAM_BYTES{counters.oversized_packets+=1;recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:id.clone(),desktop_us:ingress,category:"oversized".into(),reason:"datagram_limit".into()},false);continue}let packet=match parse_packet(&buf[..take]){Ok(v)=>v,Err(reason)=>{counters.invalid_packets+=1;recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:id.clone(),desktop_us:ingress,category:"invalid".into(),reason},false);continue}};match packet{
         LatencyPacketV1::LatencyHelloV1{measurement_id,..}=>{if id.is_none(){id=Some(measurement_id.clone());endpoint=Some(from);counters.hello_accepted=1;recorder_ingress.try_publish(LatencyRecord::Hello{measurement_protocol_version:1,measurement_id,endpoint:from.to_string(),desktop_ingress_us:ingress},true);}else{counters.duplicate_hello+=1;recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:id.clone(),desktop_us:ingress,category:"duplicate_hello".into(),reason:"hello_already_accepted".into()},false);}},
         LatencyPacketV1::ClockSyncResponseV1{measurement_id,nonce,t1_desktop_us,t2_android_us,t3_android_us,..}=>{recorder_ingress.try_publish(LatencyRecord::ClockSyncResponseReceived{measurement_protocol_version:1,measurement_id:measurement_id.clone(),nonce:nonce.clone(),endpoint:from.to_string(),t1_desktop_us,t2_android_us,t3_android_us,t4_desktop_us:ingress},true);if validator.accept(&nonce,from,&measurement_id,t1_desktop_us).is_err(){counters.probes_rejected+=1;continue}match clock_anchor(nonce,t1_desktop_us,t2_android_us,t3_android_us,ingress){Ok(a)=>{counters.probes_responded+=1;anchors.push(a);anchors.sort_by(|a,b|a.desktop_midpoint_us.total_cmp(&b.desktop_midpoint_us));},Err(_)=>{counters.probes_rejected+=1;}}},
         LatencyPacketV1::LatencySampleV1{measurement_id,android_sensor_event_us,android_send_preparation_us,session_id,sequence,sample,..}=>{if Some(from)!=endpoint||Some(&measurement_id)!=id.as_ref(){counters.foreign_packets+=1;let category=if Some(from)!=endpoint{"foreign_endpoint"}else{"foreign_measurement"};recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:id.clone(),desktop_us:ingress,category:category.into(),reason:"sample_identity_rejected".into()},false);continue}if anchors.is_empty(){recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:Some(measurement_id),desktop_us:ingress,category:"sample_before_clock_anchor".into(),reason:"measurement_not_armed".into()},false);continue}if window_done{recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:Some(measurement_id),desktop_us:ingress,category:"sample_after_window".into(),reason:"nominal_window_complete".into()},false);continue}let accepted_at=us(origin);let acceptance=receiver.apply_sample(sample.clone(),from,origin+Duration::from_micros(accepted_at));if !acceptance.was_accepted(){let category=match acceptance{SampleAcceptance::IgnoredDuplicateOrOutOfOrder=>"duplicate_or_out_of_order",SampleAcceptance::IgnoredDifferentSession=>"foreign_session",_=>"invalid"};recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:Some(measurement_id.clone()),desktop_us:ingress,category:category.into(),reason:"receiver_rejected_sample".into()},false);continue}if lifecycle.accept_sample(ingress){println!("first accepted B5b sample; nominal window started")}recorder_ingress.try_publish(LatencyRecord::SampleAccepted{measurement_protocol_version:1,measurement_id:measurement_id.clone(),session_id:session_id.clone(),sequence,sample:sample.clone(),android_sensor_event_us,android_send_preparation_us,desktop_ingress_us:ingress,desktop_accepted_us:accepted_at},true);let event=AcceptedSampleEvent{sample,sender:from,received_at:origin+Duration::from_micros(ingress)};counters.accepted_samples+=1;let raw=RawSampleV1{measurement_id,session_id,sequence,android_sensor_event_us,android_send_preparation_us,desktop_ingress_us:ingress,desktop_accepted_us:accepted_at,desktop_processed_us:None};if let Err(error)=runtime_tx.try_send(RuntimeWork{event,raw}){counters.runtime_dropped+=1;let dropped=match error{mpsc::TrySendError::Full(work)|mpsc::TrySendError::Disconnected(work)=>work.raw};recorder_ingress.try_publish(LatencyRecord::Rejection{measurement_protocol_version:1,measurement_id:Some(dropped.measurement_id),desktop_us:ingress,category:"runtime_drop".into(),reason:"bounded_runtime_queue_unavailable".into()},true);}},_=>{counters.invalid_packets+=1}}
          }}
    }
    drop(runtime_tx);
    join_runtime_worker(runtime_worker)?;
    while let Ok((raw, state)) = processed_rx.try_recv() {
        recorder_ingress.try_publish(
            LatencyRecord::SampleProcessed {
                measurement_protocol_version: 1,
                measurement_id: raw.measurement_id.clone(),
                session_id: raw.session_id.clone(),
                sequence: raw.sequence,
                desktop_processed_us: raw.desktop_processed_us.unwrap_or(0),
                processing_completed: true,
                b4b_state: state,
            },
            true,
        );
        samples.push(raw);
    }
    counters.recorder_dropped = recorder_ingress.dropped();
    counters.recorder_critical_dropped = recorder_ingress.critical_dropped();
    counters.processed_samples = processed_count.load(Ordering::Relaxed);
    counters.result_dropped = result_drops.load(Ordering::Relaxed);
    if samples.is_empty() {
        drop(recorder_ingress);
        recorder.abort()?;
        fs::remove_file(&c.dataset).map_err(|e| format!("failed to remove empty dataset: {e}"))?;
        return Err("no accepted B5b samples; empty dataset removed".into());
    }
    let receiver_metrics = receiver.snapshot(Instant::now()).metrics;
    counters.duplicate_or_out_of_order_packets = receiver_metrics.duplicate_or_out_of_order_packets;
    counters.foreign_session_packets = receiver_metrics.foreign_session_packets;
    counters.probes_lost = counters
        .probes_sent
        .saturating_sub(counters.probes_responded + counters.probes_rejected);
    recorder.shutdown(LatencyRecord::Summary {
        measurement_protocol_version: 1,
        measurement_id: id.clone().unwrap(),
        completed: !lifecycle.interrupted(),
        declared_counters: serde_json::to_value(&counters).map_err(|e| e.to_string())?,
    })?;
    let loaded = load_latency_dataset(&c.dataset)
        .map_err(|e| format!("failed to analyze B5b dataset: {e}"))?;
    let r = report_from_analysis(loaded);
    if let Some(p) = c.report.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?
    }
    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&c.report)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut out, &r).map_err(|e| e.to_string())?;
    out.write_all(b"\n").map_err(|e| e.to_string())?;
    println!("B5b integrity: {}\nfailedGates: {}\nsamples: {}\nprobes sent/responded/lost/rejected: {}/{}/{}/{}\nnetworkRttUs: {}\nsymmetricNetworkOneWayEstimateUs: {}\nsynchronizationUncertaintyUs: {}\nandroidSensorToSendUs: {}\nandroidSendToDesktopIngressEstimateUs: {}\ndesktopIngressToProcessedTiltUs: {}\nsensorToDesktopIngressEstimateUs: {}\nsensorToProcessedTiltEstimateUs: {}\nresults are approximations with synchronization uncertainty; RTT/2 assumes symmetry\nthere is no sensor-to-screen measurement and no performance verdict",if r.passed{"PASSED"}else{"FAILED"},r.gates.iter().filter(|g|!g.passed).map(|g|g.id.as_str()).collect::<Vec<_>>().join(", "),r.sample_count,r.counters.probes_sent,r.counters.probes_responded,r.counters.probes_lost,r.counters.probes_rejected,percentiles(&r.network_rtt_us),percentiles(&r.symmetric_network_one_way_estimate_us),percentiles(&r.synchronization_uncertainty_us),percentiles(&r.android_sensor_to_send_us),percentiles(&r.android_send_to_desktop_ingress_estimate_us),percentiles(&r.desktop_ingress_to_processed_tilt_us),percentiles(&r.sensor_to_desktop_ingress_estimate_us),percentiles(&r.sensor_to_processed_tilt_estimate_us));
    Ok(r.passed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_desktop_lib::{
        calibration::provenance::CalibrationProfileFingerprintV1,
        motion_filtering::{
            selection::{PolicyCandidate, POLICY_SOURCE, POLICY_V2_VERSION},
            synthetic::synthetic_suite,
        },
    };
    use std::{
        collections::BTreeMap,
        net::UdpSocket,
        sync::atomic::{AtomicU64, Ordering},
    };

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn test_config(bind: SocketAddr) -> Config {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("anchor-b5b-cli-{}-{id}", std::process::id()));
        Config {
            duration: 1,
            timeout: 1,
            final_sync_timeout: 1,
            bind,
            profile: root.join("profile.json"),
            policy: root.join("policy.json"),
            dataset: root.join("dataset.ndjson"),
            report: root.join("report.json"),
        }
    }

    fn write_valid_b4b(config: &Config) {
        fs::create_dir_all(config.profile.parent().unwrap()).unwrap();
        let profile = synthetic_suite().unwrap().remove(0).calibration_profile;
        let fingerprint = CalibrationProfileFingerprintV1::for_profile(&profile).unwrap();
        let policy = TiltEstimatorPolicyV2 {
            version: POLICY_V2_VERSION,
            candidate: PolicyCandidate::GravityNoAdditionalAnchorFilter,
            parameters: BTreeMap::new(),
            yaw_available: false,
            source: POLICY_SOURCE.into(),
            calibration_profile_fingerprint: fingerprint,
        };
        fs::write(&config.profile, serde_json::to_vec(&profile).unwrap()).unwrap();
        fs::write(&config.policy, serde_json::to_vec(&policy).unwrap()).unwrap();
    }
    #[test]
    fn config_requires_paths() {
        assert!(parse(&[]).is_err())
    }
    #[test]
    fn command_outcomes_have_real_exit_codes() {
        assert_eq!(command_exit(Ok(true)), ExitCode::SUCCESS);
        assert_eq!(command_exit(Ok(false)), ExitCode::from(2));
        assert_eq!(command_exit(Err("failure".into())), ExitCode::FAILURE);
    }
    #[test]
    fn timeout_values() {
        assert!(parse(&[
            "--profile".into(),
            "p".into(),
            "--policy".into(),
            "q".into(),
            "--startup-timeout-seconds".into(),
            "0".into()
        ])
        .is_err())
    }

    #[test]
    fn bounded_runtime_and_result_queues_report_full_and_closed() {
        let (runtime_tx, runtime_rx) = mpsc::sync_channel(1);
        runtime_tx.try_send(1_u8).unwrap();
        assert!(matches!(
            runtime_tx.try_send(2),
            Err(mpsc::TrySendError::Full(2))
        ));
        drop(runtime_rx);
        assert!(matches!(
            runtime_tx.try_send(3),
            Err(mpsc::TrySendError::Disconnected(3))
        ));

        let (result_tx, mut result_rx) = tokio::sync::mpsc::channel(1);
        result_tx.try_send(1_u8).unwrap();
        assert!(matches!(
            result_tx.try_send(2),
            Err(tokio::sync::mpsc::error::TrySendError::Full(2))
        ));
        result_rx.close();
        assert!(matches!(
            result_tx.try_send(3),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(3))
        ));
    }

    #[test]
    fn runtime_worker_panic_is_propagated() {
        let worker = thread::spawn(|| panic!("injected worker failure"));
        assert_eq!(
            join_runtime_worker(worker).unwrap_err(),
            "B4b runtime worker panicked"
        );
    }

    #[tokio::test]
    async fn existing_output_is_rejected_before_any_work() {
        let config = test_config("127.0.0.1:0".parse().unwrap());
        let root = config.dataset.parent().unwrap().to_path_buf();
        fs::create_dir_all(config.dataset.parent().unwrap()).unwrap();
        fs::write(&config.dataset, "owned").unwrap();
        let error = run(Ok(config)).await.unwrap_err();
        assert_eq!(error, "output exists; refusing silent overwrite");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn occupied_bind_does_not_create_dataset_or_report() {
        let occupied = UdpSocket::bind("127.0.0.1:0").unwrap();
        let config = test_config(occupied.local_addr().unwrap());
        let dataset = config.dataset.clone();
        let report = config.report.clone();
        let root = dataset.parent().unwrap().to_path_buf();
        let error = run(Ok(config)).await.unwrap_err();
        assert!(error.starts_with("bind failed:"));
        assert!(!dataset.exists());
        assert!(!report.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn startup_timeout_drains_workers_and_removes_empty_dataset() {
        let config = test_config("127.0.0.1:0".parse().unwrap());
        write_valid_b4b(&config);
        let dataset = config.dataset.clone();
        let report = config.report.clone();
        let root = dataset.parent().unwrap().to_path_buf();
        let error = run(Ok(config)).await.unwrap_err();
        assert!(error.starts_with("startup timeout before the first accepted B5b sample"));
        assert!(!dataset.exists());
        assert!(!report.exists());
        let _ = fs::remove_dir_all(root);
    }
}
