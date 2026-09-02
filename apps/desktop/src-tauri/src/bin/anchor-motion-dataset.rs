use anchor_desktop_lib::{
    dataset::{
        analyze_dataset_file, default_dataset_output_path, format_human_analysis,
        start_dataset_recorder, DatasetRecorderConfig, RecordingScenario,
    },
    receiver::{
        udp::{start_udp_receiver, UdpReceiverConfig},
        ReceiverMetrics, ReceiverState, SharedReceiverState,
    },
};
use std::{
    env, fs, io,
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, Mutex},
    time::{Duration, Instant as StdInstant, SystemTime},
};
use tokio::{sync::watch, time::MissedTickBehavior};

#[tokio::main]
async fn main() -> ExitCode {
    match run(env::args().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    match parse_command(&args)? {
        Command::Record {
            scenario,
            duration_seconds,
            output_path,
            bind_addr,
            notes,
        } => record_command(scenario, duration_seconds, output_path, bind_addr, notes).await,
        Command::Analyze { path, json } => analyze_command(path, json),
    }
}

#[derive(Debug)]
enum Command {
    Record {
        scenario: RecordingScenario,
        duration_seconds: u64,
        output_path: Option<PathBuf>,
        bind_addr: SocketAddr,
        notes: Option<String>,
    },
    Analyze {
        path: PathBuf,
        json: bool,
    },
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(usage());
    };

    match command {
        "record" => parse_record_command(&args[1..]),
        "analyze" => parse_analyze_command(&args[1..]),
        "--help" | "-h" => Err(usage()),
        other => Err(format!("unsupported subcommand: {other}\n\n{}", usage())),
    }
}

fn parse_record_command(args: &[String]) -> Result<Command, String> {
    let mut scenario: Option<RecordingScenario> = None;
    let mut duration_seconds: Option<u64> = None;
    let mut output_path: Option<PathBuf> = None;
    let mut bind_addr: SocketAddr = "0.0.0.0:57421"
        .parse()
        .map_err(|err| format!("invalid default bind address: {err}"))?;
    let mut notes: Option<String> = None;

    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        if flag == "--" {
            index += 1;
            continue;
        }

        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {flag}"))?;

        match flag {
            "--scenario" => {
                scenario = Some(value.parse::<RecordingScenario>()?);
            }
            "--duration-seconds" => {
                duration_seconds = Some(parse_positive_u64(value, "duration-seconds")?);
            }
            "--output" => {
                output_path = Some(PathBuf::from(value));
            }
            "--bind" => {
                bind_addr = value
                    .parse()
                    .map_err(|err| format!("invalid --bind address: {err}"))?;
            }
            "--notes" => {
                notes = Some(value.clone());
            }
            _ => {
                return Err(format!(
                    "unsupported option for record: {flag}\n\n{}",
                    usage()
                ))
            }
        }

        index += 2;
    }

    Ok(Command::Record {
        scenario: scenario.ok_or_else(|| "--scenario is required".to_owned())?,
        duration_seconds: duration_seconds
            .ok_or_else(|| "--duration-seconds is required".to_owned())?,
        output_path,
        bind_addr,
        notes,
    })
}

fn parse_analyze_command(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err(format!("analyze requires a dataset path\n\n{}", usage()));
    }

    let mut path: Option<PathBuf> = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--" => {
                index += 1;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            flag if flag.starts_with("--") => {
                return Err(format!(
                    "unsupported option for analyze: {flag}\n\n{}",
                    usage()
                ));
            }
            value => {
                if path.is_some() {
                    return Err("analyze accepts exactly one dataset path".to_owned());
                }
                path = Some(PathBuf::from(value));
                index += 1;
            }
        }
    }

    Ok(Command::Analyze {
        path: path.ok_or_else(|| "analyze requires a dataset path".to_owned())?,
        json,
    })
}

async fn record_command(
    scenario: RecordingScenario,
    duration_seconds: u64,
    output_path: Option<PathBuf>,
    bind_addr: SocketAddr,
    notes: Option<String>,
) -> Result<(), String> {
    let runtime = try_start_recording_runtime(scenario, output_path, bind_addr, notes)
        .await
        .map_err(|failure| failure.message)?;

    println!(
        "recording scenario={} for {}s on {}",
        runtime.scenario, duration_seconds, runtime.bind_addr
    );
    println!(
        "output file: {}\nclose the Anchor desktop Tauri app while using the headless recorder",
        runtime.output_path.display()
    );

    let finished_cleanly =
        wait_for_recording(duration_seconds, &runtime.shared_state, &runtime.recorder).await;

    let outcome = finalize_recording(
        runtime.receiver,
        &runtime.shared_state,
        runtime.recorder,
        &runtime.output_path,
        finished_cleanly,
    )
    .await
    .map_err(|failure| failure.message)?;

    println!(
        "dataset summary: path={} sizeBytes={} receivedAcceptedSamples={} writtenSamples={} recorderDroppedSamples={} completed={} durationUs={}",
        runtime.output_path.display(),
        outcome.size_bytes,
        outcome.summary.received_accepted_samples,
        outcome.summary.written_samples,
        outcome.summary.recorder_dropped_samples,
        outcome.summary.completed,
        outcome.summary.duration_us,
    );

    Ok(())
}

struct RecordingRuntime {
    scenario: RecordingScenario,
    output_path: PathBuf,
    bind_addr: SocketAddr,
    recorder: anchor_desktop_lib::dataset::DatasetRecorderHandle,
    receiver: anchor_desktop_lib::receiver::udp::UdpReceiverHandle,
    shared_state: SharedReceiverState,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
struct RecordingStartupFailure {
    message: String,
    recorder_shutdown_completed: bool,
    dataset_file_removed: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
struct FinalizeRecordingOutcome {
    summary: anchor_desktop_lib::dataset::DatasetSummaryRecord,
    size_bytes: u64,
    recorder_shutdown_completed: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
struct FinalizeRecordingFailure {
    message: String,
    recorder_shutdown_completed: bool,
    dataset_file_removed: bool,
}

async fn try_start_recording_runtime(
    scenario: RecordingScenario,
    output_path: Option<PathBuf>,
    bind_addr: SocketAddr,
    notes: Option<String>,
) -> Result<RecordingRuntime, RecordingStartupFailure> {
    let output_path = match output_path {
        Some(path) => path,
        None => default_dataset_output_path(scenario, SystemTime::now()).map_err(|err| {
            RecordingStartupFailure {
                message: format!("failed to build default dataset path: {err}"),
                recorder_shutdown_completed: false,
                dataset_file_removed: false,
            }
        })?,
    };

    let recorder = start_dataset_recorder(DatasetRecorderConfig {
        output_path: output_path.clone(),
        scenario,
        notes,
        queue_capacity: 256,
        flow_control: None,
    })
    .await
    .map_err(|err| RecordingStartupFailure {
        message: format!("failed to start dataset recorder: {err}"),
        recorder_shutdown_completed: false,
        dataset_file_removed: false,
    })?;

    let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
    let (sample_tx, _sample_rx) = watch::channel(None);
    let receiver = match start_udp_receiver(
        UdpReceiverConfig {
            bind_addr,
            accepted_sample_sink: Some(recorder.ingress()),
            ..UdpReceiverConfig::default()
        },
        shared_state.clone(),
        sample_tx,
    )
    .await
    {
        Ok(receiver) => receiver,
        Err(err) => {
            return Err(cleanup_failed_recording_start(
                recorder,
                &output_path,
                format_bind_error(bind_addr, &err),
            )
            .await)
        }
    };

    Ok(RecordingRuntime {
        scenario,
        output_path,
        bind_addr,
        recorder,
        receiver,
        shared_state,
    })
}

async fn finalize_recording(
    receiver: anchor_desktop_lib::receiver::udp::UdpReceiverHandle,
    shared_state: &SharedReceiverState,
    recorder: anchor_desktop_lib::dataset::DatasetRecorderHandle,
    output_path: &PathBuf,
    completed: bool,
) -> Result<FinalizeRecordingOutcome, FinalizeRecordingFailure> {
    receiver.shutdown().await;
    let receiver_metrics = shared_state
        .lock()
        .map_err(|_| FinalizeRecordingFailure {
            message: "receiver state is unavailable during dataset finalization".to_owned(),
            recorder_shutdown_completed: false,
            dataset_file_removed: false,
        })?
        .snapshot(StdInstant::now())
        .metrics;
    let summary = recorder
        .shutdown(completed)
        .await
        .map_err(|err| FinalizeRecordingFailure {
            message: format!("failed to finalize dataset: {err}"),
            recorder_shutdown_completed: false,
            dataset_file_removed: false,
        })?;

    if summary.written_samples == 0 {
        return Err(cleanup_empty_recording(
            output_path,
            &receiver_metrics,
            summary.completed,
        ));
    }

    let size_bytes = fs::metadata(output_path)
        .map_err(|err| FinalizeRecordingFailure {
            message: format!("failed to inspect dataset file: {err}"),
            recorder_shutdown_completed: true,
            dataset_file_removed: false,
        })?
        .len();
    Ok(FinalizeRecordingOutcome {
        summary,
        size_bytes,
        recorder_shutdown_completed: true,
    })
}

async fn wait_for_recording(
    duration_seconds: u64,
    shared_state: &SharedReceiverState,
    recorder: &anchor_desktop_lib::dataset::DatasetRecorderHandle,
) -> bool {
    let started_at = StdInstant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(duration_seconds);
    let mut progress_tick = tokio::time::interval(Duration::from_secs(1));
    progress_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                println!("recording duration reached");
                return true;
            }
            signal = tokio::signal::ctrl_c() => {
                match signal {
                    Ok(()) => {
                        println!("received Ctrl+C, stopping recording cleanly");
                    }
                    Err(err) => {
                        eprintln!("failed to listen for Ctrl+C: {err}");
                    }
                }
                return false;
            }
            _ = progress_tick.tick() => {
                let receiver_snapshot = shared_state
                    .lock()
                    .ok()
                    .map(|state| state.snapshot(StdInstant::now()));
                let recorder_snapshot = recorder.snapshot();
                let accepted = receiver_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.metrics.accepted_samples)
                    .unwrap_or(0);
                println!(
                    "progress: elapsedSeconds={} accepted={} written={} recorderDropped={}",
                    started_at.elapsed().as_secs(),
                    accepted,
                    recorder_snapshot.written_samples,
                    recorder_snapshot.recorder_dropped_samples,
                );
            }
        }
    }
}

fn analyze_command(path: PathBuf, json: bool) -> Result<(), String> {
    let analysis = analyze_dataset_file(&path).map_err(|err| {
        if matches!(err, anchor_desktop_lib::dataset::DatasetAnalysisError::Io(ref io_err) if io_err.kind() == io::ErrorKind::NotFound) {
            format!("dataset file does not exist: {}", path.display())
        } else {
            err.to_string()
        }
    })?;

    if json {
        let rendered = serde_json::to_string_pretty(&analysis)
            .map_err(|err| format!("failed to render JSON analysis: {err}"))?;
        println!("{rendered}");
    } else {
        println!("{}", format_human_analysis(&analysis, &path));
    }

    Ok(())
}

fn parse_positive_u64(value: &str, name: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(parsed)
}

fn format_bind_error(bind_addr: SocketAddr, err: &io::Error) -> String {
    if err.kind() == io::ErrorKind::AddrInUse {
        return format!(
            "failed to bind UDP receiver on {bind_addr}: address already in use. Close the Anchor desktop Tauri app before a headless recording."
        );
    }

    format!("failed to bind UDP receiver on {bind_addr}: {err}")
}

fn usage() -> String {
    [
        "Usage:",
        "  anchor-motion-dataset record --scenario <name> --duration-seconds <seconds> [--output <path>] [--bind <host:port>] [--notes <text>]",
        "  anchor-motion-dataset analyze <file.ndjson> [--json]",
    ]
    .join("\n")
}

async fn cleanup_failed_recording_start(
    recorder: anchor_desktop_lib::dataset::DatasetRecorderHandle,
    output_path: &PathBuf,
    startup_message: String,
) -> RecordingStartupFailure {
    let shutdown_error = recorder.shutdown(false).await.err();
    let remove_result = match fs::remove_file(output_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    };

    let mut message = startup_message;
    if let Some(err) = shutdown_error.as_ref() {
        message.push_str(&format!(
            " Cleanup also failed while stopping the dataset recorder task: {err}."
        ));
    }
    if let Err(err) = &remove_result {
        message.push_str(&format!(
            " Cleanup also failed while removing the partial dataset file {}: {err}.",
            output_path.display()
        ));
    }

    RecordingStartupFailure {
        message,
        recorder_shutdown_completed: shutdown_error.is_none(),
        dataset_file_removed: remove_result.is_ok(),
    }
}

fn cleanup_empty_recording(
    output_path: &PathBuf,
    metrics: &ReceiverMetrics,
    completed: bool,
) -> FinalizeRecordingFailure {
    let remove_result = match fs::remove_file(output_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    };

    let mut message = format!(
        "no accepted motion samples were recorded; the dataset file was not kept. Check whether the Android app is actively capturing and whether the target IP/port are correct. completed={} receivedDatagrams={} acceptedSamples={} invalidPackets={} oversizedDatagrams={} rateLimitedDatagrams={}",
        completed,
        metrics.received_datagrams,
        metrics.accepted_samples,
        metrics.invalid_packets,
        metrics.oversized_datagrams,
        metrics.rate_limited_datagrams,
    );

    if let Err(err) = &remove_result {
        message.push_str(&format!(
            " Cleanup also failed while removing the empty dataset file {}: {err}.",
            output_path.display()
        ));
    }

    FinalizeRecordingFailure {
        message,
        recorder_shutdown_completed: true,
        dataset_file_removed: remove_result.is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, time::SystemTime};
    use tokio::sync::watch;

    #[test]
    fn parse_record_command_requires_known_scenario() {
        let err = parse_command(&[
            "record".to_owned(),
            "--scenario".to_owned(),
            "unknown".to_owned(),
            "--duration-seconds".to_owned(),
            "15".to_owned(),
        ])
        .expect_err("unknown scenario must fail");

        assert!(err.contains("scenario must be one of"));
    }

    #[test]
    fn format_bind_error_mentions_tauri_when_port_is_busy() {
        let message = format_bind_error(
            "0.0.0.0:57421".parse().expect("bind addr"),
            &io::Error::new(io::ErrorKind::AddrInUse, "busy"),
        );

        assert!(message.contains("address already in use"));
        assert!(message.contains("Close the Anchor desktop Tauri app"));
    }

    #[tokio::test]
    async fn analyze_command_returns_controlled_error_for_received_elapsed_regression() {
        let path = std::env::temp_dir().join(format!(
            "anchor-analyze-received-regression-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        fs::write(&path, concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":10,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":5,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":1,\"sessionElapsedUs\":1,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        )).expect("write dataset");

        let err = run(vec!["analyze".to_owned(), path.display().to_string()])
            .await
            .expect_err("analysis must fail without panic");

        assert!(err.contains("receivedElapsedUs regressed"));
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn analyze_command_returns_controlled_error_for_received_elapsed_above_i64_max() {
        let path = std::env::temp_dir().join(format!(
            "anchor-analyze-received-range-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        fs::write(&path, concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":9223372036854775808,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        )).expect("write dataset");

        let err = run(vec!["analyze".to_owned(), path.display().to_string()])
            .await
            .expect_err("analysis must fail without panic");

        assert!(err.contains("exceeds supported analysis range"));
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn record_command_cleans_up_file_and_task_when_bind_fails() {
        let occupied = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind occupied socket");
        let bind_addr = occupied.local_addr().expect("occupied addr");
        let output_path = std::env::temp_dir().join(format!(
            "anchor-record-bind-fail-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));

        let err = match try_start_recording_runtime(
            RecordingScenario::Stationary,
            Some(output_path.clone()),
            bind_addr,
            None,
        )
        .await
        {
            Ok(_) => panic!("record startup must fail on busy port"),
            Err(err) => err,
        };

        assert!(err.message.contains("address already in use"));
        assert!(err.message.contains("Close the Anchor desktop Tauri app"));
        assert!(err.recorder_shutdown_completed);
        assert!(err.dataset_file_removed);
        assert!(!Path::new(&output_path).exists());
    }

    #[tokio::test]
    async fn cleanup_failed_recording_start_reports_local_shutdown_and_cleanup() {
        let output_path = std::env::temp_dir().join(format!(
            "anchor-recording-cleanup-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let recorder = start_dataset_recorder(DatasetRecorderConfig {
            output_path: output_path.clone(),
            scenario: RecordingScenario::Stationary,
            notes: None,
            queue_capacity: 8,
            flow_control: None,
        })
        .await
        .expect("recorder should start");

        let outcome =
            cleanup_failed_recording_start(recorder, &output_path, "startup failed".to_owned())
                .await;

        assert_eq!(outcome.message, "startup failed");
        assert!(outcome.recorder_shutdown_completed);
        assert!(outcome.dataset_file_removed);
        assert!(!Path::new(&output_path).exists());
    }

    #[tokio::test]
    async fn finalize_recording_rejects_empty_dataset_and_removes_file() {
        let output_path = std::env::temp_dir().join(format!(
            "anchor-empty-recording-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let recorder = start_dataset_recorder(DatasetRecorderConfig {
            output_path: output_path.clone(),
            scenario: RecordingScenario::Stationary,
            notes: None,
            queue_capacity: 8,
            flow_control: None,
        })
        .await
        .expect("recorder should start");
        let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
        let (sample_tx, _sample_rx) = watch::channel(None);
        let receiver = start_udp_receiver(
            UdpReceiverConfig {
                bind_addr: "127.0.0.1:0".parse().expect("bind addr"),
                accepted_sample_sink: Some(recorder.ingress()),
                ..UdpReceiverConfig::default()
            },
            shared_state.clone(),
            sample_tx,
        )
        .await
        .expect("receiver should start");

        let err = finalize_recording(receiver, &shared_state, recorder, &output_path, true)
            .await
            .expect_err("empty recording must fail");

        assert!(err
            .message
            .contains("no accepted motion samples were recorded"));
        assert!(err
            .message
            .contains("Check whether the Android app is actively capturing"));
        assert!(err.recorder_shutdown_completed);
        assert!(err.dataset_file_removed);
        assert!(!Path::new(&output_path).exists());
    }

    #[tokio::test]
    async fn finalize_recording_succeeds_with_one_accepted_sample() {
        let output_path = std::env::temp_dir().join(format!(
            "anchor-single-recording-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let recorder = start_dataset_recorder(DatasetRecorderConfig {
            output_path: output_path.clone(),
            scenario: RecordingScenario::Stationary,
            notes: None,
            queue_capacity: 8,
            flow_control: None,
        })
        .await
        .expect("recorder should start");
        let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
        let (sample_tx, _sample_rx) = watch::channel(None);
        let receiver = start_udp_receiver(
            UdpReceiverConfig {
                bind_addr: "127.0.0.1:0".parse().expect("bind addr"),
                accepted_sample_sink: Some(recorder.ingress()),
                ..UdpReceiverConfig::default()
            },
            shared_state.clone(),
            sample_tx,
        )
        .await
        .expect("receiver should start");

        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("sender bind");
        socket
            .send_to(
                include_str!("../../../../../packages/protocol/fixtures/valid/motion-sample.json")
                    .as_bytes(),
                receiver.local_addr(),
            )
            .await
            .expect("send valid datagram");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if recorder.snapshot().written_samples == 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "sample was not written in time"
            );
            tokio::task::yield_now().await;
        }

        let outcome = finalize_recording(receiver, &shared_state, recorder, &output_path, true)
            .await
            .expect("single-sample recording should succeed");

        assert_eq!(outcome.summary.written_samples, 1);
        assert!(outcome.size_bytes > 0);
        assert!(outcome.recorder_shutdown_completed);
        let _ = fs::remove_file(output_path);
    }
}
