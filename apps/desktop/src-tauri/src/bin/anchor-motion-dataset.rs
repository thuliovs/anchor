use anchor_desktop_lib::{
    calibration::{
        calibrate_dataset_file, default_profile_output_path_for_dataset, format_human_report,
        write_calibration_profile_file,
    },
    dataset::{
        analyze_dataset_file, default_dataset_output_path, format_human_analysis,
        start_dataset_recorder, DatasetRecorderConfig, RecordingScenario,
    },
    motion_filtering::{
        evaluate_dataset_file, evaluate_synthetic_suite, format_human_evaluation,
        selection::{
            format_human_selection, select_tilt_estimator, PhysicalDatasetSelectionInput,
            SelectionConfig, DEFAULT_SELECTION_COMPLEMENTARY_TAU_MS,
            DEFAULT_SELECTION_LOW_PASS_TAU_MS,
        },
        EvaluationConfig,
    },
    receiver::{
        udp::{start_udp_receiver, UdpReceiverConfig},
        ReceiverMetrics, ReceiverState, SharedReceiverState,
    },
    transport_measurement::{
        build_report_with_arming_counters, format_human_report as format_human_measurement_report,
        ReceiverCounters, DEFAULT_MEASUREMENT_DIRECTORY, DEFAULT_MEASUREMENT_DURATION_SECONDS,
        DEFAULT_STARTUP_TIMEOUT_SECONDS,
    },
};
use std::{
    env, fs,
    future::Future,
    io,
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
        Ok(outcome) => exit_code_for_outcome(outcome),
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn exit_code_for_outcome(outcome: CommandOutcome) -> ExitCode {
    match outcome {
        CommandOutcome::Success => ExitCode::SUCCESS,
        CommandOutcome::MeasurementFailedGates => ExitCode::from(2),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandOutcome {
    Success,
    MeasurementFailedGates,
}

async fn run(args: Vec<String>) -> Result<CommandOutcome, String> {
    match parse_command(&args)? {
        Command::Record {
            scenario,
            duration_seconds,
            output_path,
            bind_addr,
            notes,
        } => record_command(scenario, duration_seconds, output_path, bind_addr, notes)
            .await
            .map(|_| CommandOutcome::Success),
        Command::Measure {
            duration_seconds,
            startup_timeout_seconds,
            dataset_path,
            report_path,
            bind_addr,
        } => {
            measure_command(
                duration_seconds,
                startup_timeout_seconds,
                dataset_path,
                report_path,
                bind_addr,
            )
            .await
        }
        Command::Analyze { path, json } => {
            analyze_command(path, json).map(|_| CommandOutcome::Success)
        }
        Command::Calibrate {
            path,
            output_path,
            json,
        } => calibrate_command(path, output_path, json).map(|_| CommandOutcome::Success),
        Command::Evaluate {
            synthetic,
            dataset_path,
            profile_path,
            low_pass_tau_ms,
            complementary_tau_ms,
            json,
        } => evaluate_command(
            synthetic,
            dataset_path,
            profile_path,
            low_pass_tau_ms,
            complementary_tau_ms,
            json,
        )
        .map(|_| CommandOutcome::Success),
        Command::Select {
            profile_path,
            datasets,
            low_pass_tau_ms,
            complementary_tau_ms,
            json,
        } => select_command(
            profile_path,
            datasets,
            low_pass_tau_ms,
            complementary_tau_ms,
            json,
        )
        .map(|_| CommandOutcome::Success),
    }
}

#[derive(Debug)]
enum Command {
    Measure {
        duration_seconds: u64,
        startup_timeout_seconds: u64,
        dataset_path: Option<PathBuf>,
        report_path: Option<PathBuf>,
        bind_addr: SocketAddr,
    },
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
    Calibrate {
        path: PathBuf,
        output_path: Option<PathBuf>,
        json: bool,
    },
    Evaluate {
        synthetic: bool,
        dataset_path: Option<PathBuf>,
        profile_path: Option<PathBuf>,
        low_pass_tau_ms: Vec<f64>,
        complementary_tau_ms: Vec<f64>,
        json: bool,
    },
    Select {
        profile_path: PathBuf,
        datasets: Vec<PhysicalDatasetSelectionInput>,
        low_pass_tau_ms: Vec<f64>,
        complementary_tau_ms: Vec<f64>,
        json: bool,
    },
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(usage());
    };

    match command {
        "record" => parse_record_command(&args[1..]),
        "measure" => parse_measure_command(&args[1..]),
        "analyze" => parse_analyze_command(&args[1..]),
        "calibrate" => parse_calibrate_command(&args[1..]),
        "evaluate" => parse_evaluate_command(&args[1..]),
        "select" => parse_select_command(&args[1..]),
        "--help" | "-h" => Err(usage()),
        other => Err(format!("unsupported subcommand: {other}\n\n{}", usage())),
    }
}

fn parse_measure_command(args: &[String]) -> Result<Command, String> {
    let mut duration_seconds = DEFAULT_MEASUREMENT_DURATION_SECONDS;
    let mut startup_timeout_seconds = DEFAULT_STARTUP_TIMEOUT_SECONDS;
    let mut dataset_path = None;
    let mut report_path = None;
    let mut bind_addr = "0.0.0.0:57421"
        .parse()
        .map_err(|err| format!("invalid default bind address: {err}"))?;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--" {
            index += 1;
            continue;
        }
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag {
            "--duration-seconds" => {
                duration_seconds = parse_positive_u64(value, "duration-seconds")?
            }
            "--startup-timeout-seconds" => {
                startup_timeout_seconds = parse_positive_u64(value, "startup-timeout-seconds")?
            }
            "--dataset-output" => {
                if dataset_path.is_some() {
                    return Err("--dataset-output may only be provided once".to_owned());
                }
                dataset_path = Some(PathBuf::from(value));
            }
            "--report-output" => {
                if report_path.is_some() {
                    return Err("--report-output may only be provided once".to_owned());
                }
                report_path = Some(PathBuf::from(value));
            }
            "--bind" => {
                bind_addr = value
                    .parse()
                    .map_err(|err| format!("invalid --bind address: {err}"))?
            }
            _ => {
                return Err(format!(
                    "unsupported option for measure: {flag}\n\n{}",
                    usage()
                ))
            }
        }
        index += 2;
    }
    Ok(Command::Measure {
        duration_seconds,
        startup_timeout_seconds,
        dataset_path,
        report_path,
        bind_addr,
    })
}

fn parse_select_command(args: &[String]) -> Result<Command, String> {
    let mut profile_path: Option<PathBuf> = None;
    let mut datasets = Vec::new();
    let mut low_pass_tau_ms: Option<Vec<f64>> = None;
    let mut complementary_tau_ms: Option<Vec<f64>> = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--" => index += 1,
            "--json" => {
                if json {
                    return Err("--json may only be provided once".to_owned());
                }
                json = true;
                index += 1;
            }
            "--profile" => {
                if profile_path.is_some() {
                    return Err("--profile may only be provided once".to_owned());
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --profile".to_owned())?;
                profile_path = Some(PathBuf::from(value));
                index += 2;
            }
            "--dataset" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --dataset".to_owned())?;
                let (scenario, path) = parse_scenario_dataset(value)?;
                datasets.push(PhysicalDatasetSelectionInput { scenario, path });
                index += 2;
            }
            "--low-pass-tau-ms" => {
                if low_pass_tau_ms.is_some() {
                    return Err("--low-pass-tau-ms may only be provided once".to_owned());
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --low-pass-tau-ms".to_owned())?;
                low_pass_tau_ms = Some(parse_tau_csv(value, "--low-pass-tau-ms")?);
                index += 2;
            }
            "--complementary-tau-ms" => {
                if complementary_tau_ms.is_some() {
                    return Err("--complementary-tau-ms may only be provided once".to_owned());
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --complementary-tau-ms".to_owned())?;
                complementary_tau_ms = Some(parse_tau_csv(value, "--complementary-tau-ms")?);
                index += 2;
            }
            flag if flag.starts_with("--") => {
                return Err(format!(
                    "unsupported option for select: {flag}\n\n{}",
                    usage()
                ))
            }
            value => {
                return Err(format!(
                    "unexpected positional argument for select: {value}"
                ))
            }
        }
    }
    let profile_path = profile_path.ok_or_else(|| "select requires --profile".to_owned())?;
    let low_pass_tau_ms = low_pass_tau_ms.unwrap_or(DEFAULT_SELECTION_LOW_PASS_TAU_MS.to_vec());
    let complementary_tau_ms =
        complementary_tau_ms.unwrap_or(DEFAULT_SELECTION_COMPLEMENTARY_TAU_MS.to_vec());
    SelectionConfig::new(
        profile_path.clone(),
        datasets.clone(),
        low_pass_tau_ms.clone(),
        complementary_tau_ms.clone(),
    )
    .map_err(|err| err.to_string())?;
    Ok(Command::Select {
        profile_path,
        datasets,
        low_pass_tau_ms,
        complementary_tau_ms,
        json,
    })
}

fn parse_evaluate_command(args: &[String]) -> Result<Command, String> {
    let mut synthetic = false;
    let mut dataset_path: Option<PathBuf> = None;
    let mut profile_path: Option<PathBuf> = None;
    let mut low_pass_tau_ms: Option<Vec<f64>> = None;
    let mut complementary_tau_ms: Option<Vec<f64>> = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--" => index += 1,
            "--synthetic" => {
                if synthetic {
                    return Err("--synthetic may only be provided once".to_owned());
                }
                synthetic = true;
                index += 1;
            }
            "--json" => {
                if json {
                    return Err("--json may only be provided once".to_owned());
                }
                json = true;
                index += 1;
            }
            "--dataset" => {
                if dataset_path.is_some() {
                    return Err("--dataset may only be provided once".to_owned());
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --dataset".to_owned())?;
                dataset_path = Some(PathBuf::from(value));
                index += 2;
            }
            "--profile" => {
                if profile_path.is_some() {
                    return Err("--profile may only be provided once".to_owned());
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --profile".to_owned())?;
                profile_path = Some(PathBuf::from(value));
                index += 2;
            }
            "--low-pass-tau-ms" => {
                if low_pass_tau_ms.is_some() {
                    return Err("--low-pass-tau-ms may only be provided once".to_owned());
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --low-pass-tau-ms".to_owned())?;
                low_pass_tau_ms = Some(parse_tau_csv(value, "--low-pass-tau-ms")?);
                index += 2;
            }
            "--complementary-tau-ms" => {
                if complementary_tau_ms.is_some() {
                    return Err("--complementary-tau-ms may only be provided once".to_owned());
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --complementary-tau-ms".to_owned())?;
                complementary_tau_ms = Some(parse_tau_csv(value, "--complementary-tau-ms")?);
                index += 2;
            }
            flag if flag.starts_with("--") => {
                return Err(format!(
                    "unsupported option for evaluate: {flag}\n\n{}",
                    usage()
                ))
            }
            value => {
                return Err(format!(
                    "unexpected positional argument for evaluate: {value}"
                ))
            }
        }
    }
    if synthetic && dataset_path.is_some() {
        return Err("--synthetic is mutually exclusive with --dataset".to_owned());
    }
    if !synthetic && dataset_path.is_none() {
        return Err("evaluate requires either --synthetic or --dataset".to_owned());
    }
    if dataset_path.is_some() && profile_path.is_none() {
        return Err("--dataset requires --profile".to_owned());
    }
    if profile_path.is_some() && dataset_path.is_none() {
        return Err("--profile requires --dataset".to_owned());
    }

    let default = EvaluationConfig::default();
    let low_pass_tau_ms = low_pass_tau_ms.unwrap_or(default.low_pass_tau_ms);
    let complementary_tau_ms = complementary_tau_ms.unwrap_or(default.complementary_tau_ms);
    EvaluationConfig::new(low_pass_tau_ms.clone(), complementary_tau_ms.clone())
        .map_err(|err| err.to_string())?;
    Ok(Command::Evaluate {
        synthetic,
        dataset_path,
        profile_path,
        low_pass_tau_ms,
        complementary_tau_ms,
        json,
    })
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

fn parse_calibrate_command(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err(format!("calibrate requires a dataset path\n\n{}", usage()));
    }

    let mut path: Option<PathBuf> = None;
    let mut output_path: Option<PathBuf> = None;
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
            "--output" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --output".to_owned())?;
                output_path = Some(PathBuf::from(value));
                index += 2;
            }
            flag if flag.starts_with("--") => {
                return Err(format!(
                    "unsupported option for calibrate: {flag}\n\n{}",
                    usage()
                ));
            }
            value => {
                if path.is_some() {
                    return Err("calibrate accepts exactly one dataset path".to_owned());
                }
                path = Some(PathBuf::from(value));
                index += 1;
            }
        }
    }

    Ok(Command::Calibrate {
        path: path.ok_or_else(|| "calibrate requires a dataset path".to_owned())?,
        output_path,
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
    sample_rx: watch::Receiver<Option<anchor_desktop_lib::protocol::MotionSampleV1>>,
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
    receiver_metrics: ReceiverMetrics,
    measurement_window_baseline: Option<ReceiverMetrics>,
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
    let (sample_tx, sample_rx) = watch::channel(None);
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
        sample_rx,
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
    let (receiver_metrics, measurement_window_baseline) = {
        let state = shared_state.lock().map_err(|_| FinalizeRecordingFailure {
            message: "receiver state is unavailable during dataset finalization".to_owned(),
            recorder_shutdown_completed: false,
            dataset_file_removed: false,
        })?;
        (
            state.snapshot(StdInstant::now()).metrics,
            state.measurement_window_baseline(),
        )
    };
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
        receiver_metrics,
        measurement_window_baseline,
    })
}

async fn measure_command(
    duration_seconds: u64,
    startup_timeout_seconds: u64,
    dataset_path: Option<PathBuf>,
    report_path: Option<PathBuf>,
    bind_addr: SocketAddr,
) -> Result<CommandOutcome, String> {
    let dataset_path = match dataset_path {
        Some(path) => path,
        None => default_dataset_output_path(RecordingScenario::Stationary, SystemTime::now())
            .map_err(|err| format!("failed to build default dataset path: {err}"))?,
    };
    let report_path = report_path.unwrap_or_else(|| {
        let stem = dataset_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("transport-measurement");
        PathBuf::from(DEFAULT_MEASUREMENT_DIRECTORY).join(format!("{stem}-b5a-v1.json"))
    });
    if report_path.exists() {
        return Err(format!(
            "report output already exists; refusing to overwrite: {}",
            report_path.display()
        ));
    }
    println!("transport measurement B5a\nbind: {bind_addr}\ndurationSeconds: {duration_seconds}\nstartupTimeoutSeconds: {startup_timeout_seconds}\ndataset: {}\nreport: {}\nclose the Anchor desktop Tauri app before starting", dataset_path.display(), report_path.display());
    let mut runtime = try_start_recording_runtime(
        RecordingScenario::Stationary,
        Some(dataset_path.clone()),
        bind_addr,
        Some("B5a controlled transport measurement".to_owned()),
    )
    .await
    .map_err(|failure| failure.message)?;
    println!("receiver ready; waiting for the first accepted sample before starting the measurement window");
    let wait_outcome = wait_for_measurement(
        Duration::from_secs(startup_timeout_seconds),
        Duration::from_secs(duration_seconds),
        &runtime.shared_state,
        &runtime.recorder,
        &mut runtime.sample_rx,
        async {
            if let Err(err) = tokio::signal::ctrl_c().await {
                eprintln!("failed to listen for Ctrl+C: {err}");
            }
        },
    )
    .await;
    let completed = wait_outcome == MeasurementWaitOutcome::Completed;
    let outcome = finalize_recording(
        runtime.receiver,
        &runtime.shared_state,
        runtime.recorder,
        &dataset_path,
        completed,
    )
    .await;
    if matches!(
        wait_outcome,
        MeasurementWaitOutcome::StartupTimeout
            | MeasurementWaitOutcome::InterruptedBeforeFirstSample
    ) {
        let cleanup_message = outcome
            .err()
            .map(|failure| failure.message)
            .unwrap_or_else(|| "measurement stopped before its first accepted sample".to_owned());
        return Err(match wait_outcome {
            MeasurementWaitOutcome::StartupTimeout => format!(
                "startup timeout expired after {startup_timeout_seconds}s while waiting for the first accepted sample. {cleanup_message}"
            ),
            MeasurementWaitOutcome::InterruptedBeforeFirstSample => format!(
                "measurement interrupted before the first accepted sample. {cleanup_message}"
            ),
            _ => unreachable!(),
        });
    }
    let outcome = outcome.map_err(|failure| failure.message)?;
    let analysis = analyze_dataset_file(&dataset_path)
        .map_err(|err| format!("failed to analyze completed measurement dataset: {err}"))?;
    if analysis.source_interval_us.count == 0 || analysis.receive_interval_us.count == 0 {
        return Err("measurement has insufficient samples to calculate temporal intervals; dataset was preserved".to_owned());
    }
    let window_baseline = outcome
        .measurement_window_baseline
        .as_ref()
        .ok_or_else(|| {
            "measurement window baseline is unavailable after the first accepted sample".to_owned()
        })?;
    let arming_receiver_counters =
        ReceiverCounters::delta(&ReceiverMetrics::default(), window_baseline);
    let receiver_counters = ReceiverCounters::delta(window_baseline, &outcome.receiver_metrics);
    let report = build_report_with_arming_counters(
        &analysis,
        &outcome.summary,
        duration_seconds.saturating_mul(1_000_000),
        anchor_desktop_lib::receiver::EXPECTED_SAMPLE_RATE_HZ,
        arming_receiver_counters,
        receiver_counters,
    );
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create report directory: {err}"))?;
    }
    let mut report_file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&report_path)
        .map_err(|err| format!("failed to create report without overwriting: {err}"))?;
    serde_json::to_writer_pretty(&mut report_file, &report)
        .map_err(|err| format!("failed to write report JSON: {err}"))?;
    use std::io::Write;
    report_file
        .write_all(b"\n")
        .map_err(|err| format!("failed to finish report JSON: {err}"))?;
    println!("{}", format_human_measurement_report(&report));
    println!(
        "dataset preserved: {}\nreport written: {}",
        dataset_path.display(),
        report_path.display()
    );
    Ok(if report.integrity_evaluation.passed {
        CommandOutcome::Success
    } else {
        CommandOutcome::MeasurementFailedGates
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasurementWaitOutcome {
    Completed,
    StartupTimeout,
    InterruptedBeforeFirstSample,
    InterruptedDuringWindow,
}

async fn wait_for_measurement<F>(
    startup_timeout: Duration,
    measurement_duration: Duration,
    shared_state: &SharedReceiverState,
    recorder: &anchor_desktop_lib::dataset::DatasetRecorderHandle,
    sample_rx: &mut watch::Receiver<Option<anchor_desktop_lib::protocol::MotionSampleV1>>,
    interrupt: F,
) -> MeasurementWaitOutcome
where
    F: Future<Output = ()>,
{
    tokio::pin!(interrupt);
    let startup_deadline = tokio::time::Instant::now() + startup_timeout;
    let mut progress_tick = tokio::time::interval(Duration::from_secs(1));
    progress_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    if sample_rx.borrow().is_none() {
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(startup_deadline) => {
                    println!("startup timeout reached while waiting for the first accepted sample");
                    return MeasurementWaitOutcome::StartupTimeout;
                }
                _ = &mut interrupt => {
                    println!("received Ctrl+C before the first accepted sample");
                    return MeasurementWaitOutcome::InterruptedBeforeFirstSample;
                }
                changed = sample_rx.changed() => {
                    if changed.is_err() {
                        return MeasurementWaitOutcome::InterruptedBeforeFirstSample;
                    }
                    if sample_rx.borrow_and_update().is_some() {
                        break;
                    }
                }
                _ = progress_tick.tick() => {
                    println!("waiting for stream: accepted=0 written={} recorderDropped={}", recorder.snapshot().written_samples, recorder.snapshot().recorder_dropped_samples);
                }
            }
        }
    }

    println!("first accepted sample received; measurement window started");
    let window_started_at = StdInstant::now();
    let deadline = tokio::time::Instant::now() + measurement_duration;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                println!("measurement duration reached");
                return MeasurementWaitOutcome::Completed;
            }
            _ = &mut interrupt => {
                println!("received Ctrl+C during measurement window");
                return MeasurementWaitOutcome::InterruptedDuringWindow;
            }
            _ = progress_tick.tick() => {
                let accepted = shared_state.lock().ok().map(|state| state.snapshot(StdInstant::now()).metrics.accepted_samples).unwrap_or(0);
                let recorder_snapshot = recorder.snapshot();
                println!("measurement progress: elapsedSeconds={} accepted={} written={} recorderDropped={}", window_started_at.elapsed().as_secs(), accepted, recorder_snapshot.written_samples, recorder_snapshot.recorder_dropped_samples);
            }
        }
    }
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

fn calibrate_command(
    path: PathBuf,
    output_path: Option<PathBuf>,
    json: bool,
) -> Result<(), String> {
    let report = calibrate_dataset_file(&path).map_err(|err| match &err {
        anchor_desktop_lib::calibration::CalibrationError::Dataset(dataset_err) => {
            if matches!(dataset_err, anchor_desktop_lib::dataset::DatasetAnalysisError::Io(ref io_err) if io_err.kind() == io::ErrorKind::NotFound)
            {
                format!("dataset file does not exist: {}", path.display())
            } else {
                err.to_string()
            }
        }
        _ => err.to_string(),
    })?;

    let output_path = output_path.unwrap_or_else(|| default_profile_output_path_for_dataset(&path));
    write_calibration_profile_file(&output_path, &report.profile).map_err(|err| err.to_string())?;

    if json {
        let rendered = serde_json::to_string_pretty(&serde_json::json!({
            "profilePath": output_path,
            "profile": report.profile,
            "residuals": report.residuals,
        }))
        .map_err(|err| format!("failed to render calibration JSON: {err}"))?;
        println!("{rendered}");
    } else {
        println!("{}", format_human_report(&report, &path));
        println!("written profile: {}", output_path.display());
    }

    Ok(())
}

fn evaluate_command(
    synthetic: bool,
    dataset_path: Option<PathBuf>,
    profile_path: Option<PathBuf>,
    low_pass_tau_ms: Vec<f64>,
    complementary_tau_ms: Vec<f64>,
    json: bool,
) -> Result<(), String> {
    let config = EvaluationConfig::new(low_pass_tau_ms, complementary_tau_ms)
        .map_err(|err| err.to_string())?;
    let report = if synthetic {
        evaluate_synthetic_suite(&config).map_err(|err| err.to_string())?
    } else {
        let dataset = dataset_path.ok_or_else(|| "--dataset is required".to_owned())?;
        let profile = profile_path.ok_or_else(|| "--profile is required".to_owned())?;
        evaluate_dataset_file(&dataset, &profile, &config).map_err(|err| match &err {
            anchor_desktop_lib::motion_filtering::EvaluationError::Dataset(dataset_err) => {
                if matches!(dataset_err, anchor_desktop_lib::dataset::DatasetAnalysisError::Io(ref io_err) if io_err.kind() == io::ErrorKind::NotFound) { format!("dataset file does not exist: {}", dataset.display()) } else { err.to_string() }
            }
            anchor_desktop_lib::motion_filtering::EvaluationError::Calibration(anchor_desktop_lib::calibration::CalibrationError::Io(io_err)) if io_err.kind() == io::ErrorKind::NotFound => format!("profile file does not exist: {}", profile.display()),
            _ => err.to_string(),
        })?
    };

    if json {
        let rendered = serde_json::to_string_pretty(&report)
            .map_err(|err| format!("failed to render evaluation JSON: {err}"))?;
        println!("{rendered}");
    } else {
        println!("{}", format_human_evaluation(&report));
    }
    Ok(())
}

fn select_command(
    profile_path: PathBuf,
    datasets: Vec<PhysicalDatasetSelectionInput>,
    low_pass_tau_ms: Vec<f64>,
    complementary_tau_ms: Vec<f64>,
    json: bool,
) -> Result<(), String> {
    let config = SelectionConfig::new(
        profile_path.clone(),
        datasets,
        low_pass_tau_ms,
        complementary_tau_ms,
    )
    .map_err(|err| err.to_string())?;
    let report = select_tilt_estimator(config).map_err(|err| match &err {
        anchor_desktop_lib::motion_filtering::selection::SelectionError::Evaluation(
            anchor_desktop_lib::motion_filtering::EvaluationError::Dataset(dataset_err),
        ) => {
            if matches!(dataset_err, anchor_desktop_lib::dataset::DatasetAnalysisError::Io(ref io_err) if io_err.kind() == io::ErrorKind::NotFound)
            {
                "dataset file does not exist for one of the selected scenarios".to_owned()
            } else {
                err.to_string()
            }
        }
        anchor_desktop_lib::motion_filtering::selection::SelectionError::Evaluation(
            anchor_desktop_lib::motion_filtering::EvaluationError::Calibration(
                anchor_desktop_lib::calibration::CalibrationError::Io(io_err),
            ),
        ) if io_err.kind() == io::ErrorKind::NotFound => {
            format!("profile file does not exist: {}", profile_path.display())
        }
        _ => err.to_string(),
    })?;

    if json {
        let rendered = serde_json::to_string_pretty(&report)
            .map_err(|err| format!("failed to render selection JSON: {err}"))?;
        println!("{rendered}");
    } else {
        println!("{}", format_human_selection(&report));
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

fn parse_tau_csv(value: &str, flag: &str) -> Result<Vec<f64>, String> {
    if value.is_empty() {
        return Err(format!("{flag} must not be empty"));
    }
    let mut out = Vec::new();
    for token in value.split(',') {
        if token.is_empty() {
            return Err(format!("{flag} contains an empty value"));
        }
        if !token.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
            return Err(format!("{flag} contains invalid numeric token: {token}"));
        }
        let parsed = token
            .parse::<f64>()
            .map_err(|_| format!("{flag} contains invalid numeric token: {token}"))?;
        if format_tau_token(parsed) != token.trim_end_matches(".0")
            && !(token.ends_with(".0") && format_tau_token(parsed) == token.trim_end_matches(".0"))
        {
            return Err(format!("{flag} contains partially numeric token: {token}"));
        }
        out.push(parsed);
    }
    EvaluationConfig::new(out.clone(), vec![1.0]).map_err(|err| err.to_string())?;
    Ok(out)
}

fn parse_scenario_dataset(value: &str) -> Result<(RecordingScenario, PathBuf), String> {
    let Some((scenario, path)) = value.split_once('=') else {
        return Err("--dataset must use scenario=path".to_owned());
    };
    if path.is_empty() {
        return Err("--dataset path must not be empty".to_owned());
    }
    let scenario = scenario.parse::<RecordingScenario>()?;
    Ok((scenario, PathBuf::from(path)))
}

fn format_tau_token(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
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
        "  anchor-motion-dataset measure [--duration-seconds <seconds>] [--startup-timeout-seconds <seconds>] [--dataset-output <path>] [--report-output <path>] [--bind <host:port>]",
        "  anchor-motion-dataset record --scenario <name> --duration-seconds <seconds> [--output <path>] [--bind <host:port>] [--notes <text>]",
        "  anchor-motion-dataset analyze <file.ndjson> [--json]",
        "  anchor-motion-dataset calibrate <file.ndjson> [--output <profile.json>] [--json]",
        "  anchor-motion-dataset evaluate (--synthetic | --dataset <file.ndjson> --profile <profile.json>) [--low-pass-tau-ms <csv>] [--complementary-tau-ms <csv>] [--json]",
        "  anchor-motion-dataset select --profile <profile.json> --dataset <scenario=file.ndjson>... [--low-pass-tau-ms <csv>] [--complementary-tau-ms <csv>] [--json]",
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
    use anchor_desktop_lib::transport_measurement::build_report;
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
    fn parse_measure_rejects_invalid_duration_without_panic() {
        for value in ["0", "-1", "abc"] {
            let result = std::panic::catch_unwind(|| {
                parse_command(&[
                    "measure".to_owned(),
                    "--duration-seconds".to_owned(),
                    value.to_owned(),
                ])
            });
            assert!(result.expect("parser must not panic").is_err());
        }
    }

    #[test]
    fn parse_measure_accepts_startup_timeout_and_rejects_invalid_values() {
        let parsed = parse_command(&[
            "measure".to_owned(),
            "--startup-timeout-seconds".to_owned(),
            "7".to_owned(),
        ])
        .expect("valid startup timeout");
        assert!(matches!(
            parsed,
            Command::Measure {
                startup_timeout_seconds: 7,
                ..
            }
        ));

        for value in ["0", "-1", "invalid"] {
            let result = std::panic::catch_unwind(|| {
                parse_command(&[
                    "measure".to_owned(),
                    "--startup-timeout-seconds".to_owned(),
                    value.to_owned(),
                ])
            });
            assert!(result.expect("parser must not panic").is_err());
        }
    }

    fn measurement_payload(sequence: u32, elapsed_us: u64) -> Vec<u8> {
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../packages/protocol/fixtures/valid/motion-sample.json"
        ))
        .expect("valid fixture");
        value["sequence"] = serde_json::json!(sequence);
        value["sessionElapsedUs"] = serde_json::json!(elapsed_us);
        serde_json::to_vec(&value).expect("serialize payload")
    }

    #[tokio::test]
    async fn measurement_window_waits_for_first_sample_and_keeps_it_in_dataset() {
        let output_path = std::env::temp_dir().join(format!(
            "anchor-measurement-window-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let mut runtime = try_start_recording_runtime(
            RecordingScenario::Stationary,
            Some(output_path.clone()),
            "127.0.0.1:0".parse().unwrap(),
            None,
        )
        .await
        .expect("runtime");
        let target = runtime.receiver.local_addr();
        let sender = tokio::spawn(async move {
            let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            socket.send_to(b"{", target).await.unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
            socket
                .send_to(&measurement_payload(42, 0), target)
                .await
                .unwrap();
            socket
                .send_to(&measurement_payload(43, 10_000), target)
                .await
                .unwrap();
            socket
                .send_to(&measurement_payload(44, 20_000), target)
                .await
                .unwrap();
        });
        let started = StdInstant::now();
        let wait = wait_for_measurement(
            Duration::from_millis(200),
            Duration::from_millis(40),
            &runtime.shared_state,
            &runtime.recorder,
            &mut runtime.sample_rx,
            std::future::pending(),
        )
        .await;
        assert_eq!(wait, MeasurementWaitOutcome::Completed);
        assert!(started.elapsed() >= Duration::from_millis(70));
        sender.await.unwrap();
        let finalized = finalize_recording(
            runtime.receiver,
            &runtime.shared_state,
            runtime.recorder,
            &output_path,
            true,
        )
        .await
        .expect("finalize");
        let analysis = anchor_desktop_lib::dataset::load_validated_dataset_file(&output_path)
            .expect("dataset");
        assert_eq!(analysis.samples.len(), 3);
        assert_eq!(analysis.samples[0].sample.sequence, 42);
        let baseline = finalized.measurement_window_baseline.as_ref().unwrap();
        let arming = ReceiverCounters::delta(&ReceiverMetrics::default(), baseline);
        let window = ReceiverCounters::delta(baseline, &finalized.receiver_metrics);
        assert_eq!(arming.received_datagrams, 1);
        assert_eq!(arming.invalid_packets, 1);
        assert_eq!(arming.accepted_samples, 0);
        assert_eq!(window.received_datagrams, 3);
        assert_eq!(window.accepted_samples, 3);
        assert_eq!(window.invalid_packets, 0);
        let report = build_report_with_arming_counters(
            &analyze_dataset_file(&output_path).unwrap(),
            &finalized.summary,
            40_000,
            60,
            arming,
            window,
        );
        assert_eq!(report.arming_receiver_counters.invalid_packets, 1);
        assert_eq!(report.receiver_counters.invalid_packets, 0);
        assert_eq!(
            report
                .integrity_evaluation
                .gates
                .iter()
                .find(|gate| gate.id == "receiver.invalid_zero")
                .unwrap()
                .result,
            anchor_desktop_lib::transport_measurement::GateResult::Passed
        );
        let _ = fs::remove_file(output_path);
    }

    #[tokio::test]
    async fn invalid_datagram_after_first_sample_is_inside_window_and_fails_gate() {
        let output_path = std::env::temp_dir().join(format!(
            "anchor-measurement-window-invalid-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let mut runtime = try_start_recording_runtime(
            RecordingScenario::Stationary,
            Some(output_path.clone()),
            "127.0.0.1:0".parse().unwrap(),
            None,
        )
        .await
        .unwrap();
        let target = runtime.receiver.local_addr();
        tokio::spawn(async move {
            let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            socket
                .send_to(&measurement_payload(42, 0), target)
                .await
                .unwrap();
            socket.send_to(b"{", target).await.unwrap();
            socket
                .send_to(&measurement_payload(43, 16_667), target)
                .await
                .unwrap();
        });
        assert_eq!(
            wait_for_measurement(
                Duration::from_secs(1),
                Duration::from_millis(50),
                &runtime.shared_state,
                &runtime.recorder,
                &mut runtime.sample_rx,
                std::future::pending(),
            )
            .await,
            MeasurementWaitOutcome::Completed
        );
        let finalized = finalize_recording(
            runtime.receiver,
            &runtime.shared_state,
            runtime.recorder,
            &output_path,
            true,
        )
        .await
        .unwrap();
        let baseline = finalized.measurement_window_baseline.as_ref().unwrap();
        let window = ReceiverCounters::delta(baseline, &finalized.receiver_metrics);
        assert_eq!(window.invalid_packets, 1);
        let report = build_report(
            &analyze_dataset_file(&output_path).unwrap(),
            &finalized.summary,
            50_000,
            60,
            window,
        );
        assert_eq!(
            report
                .integrity_evaluation
                .gates
                .iter()
                .find(|gate| gate.id == "receiver.invalid_zero")
                .unwrap()
                .result,
            anchor_desktop_lib::transport_measurement::GateResult::Failed
        );
        let _ = fs::remove_file(output_path);
    }

    #[tokio::test]
    async fn startup_timeout_shuts_down_and_removes_empty_dataset() {
        let output_path = std::env::temp_dir().join(format!(
            "anchor-measurement-timeout-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let mut runtime = try_start_recording_runtime(
            RecordingScenario::Stationary,
            Some(output_path.clone()),
            "127.0.0.1:0".parse().unwrap(),
            None,
        )
        .await
        .expect("runtime");
        let wait = wait_for_measurement(
            Duration::from_millis(20),
            Duration::from_secs(1),
            &runtime.shared_state,
            &runtime.recorder,
            &mut runtime.sample_rx,
            std::future::pending(),
        )
        .await;
        assert_eq!(wait, MeasurementWaitOutcome::StartupTimeout);
        let failure = finalize_recording(
            runtime.receiver,
            &runtime.shared_state,
            runtime.recorder,
            &output_path,
            false,
        )
        .await
        .expect_err("empty recording must be cleaned up");
        assert!(failure.recorder_shutdown_completed);
        assert!(failure.dataset_file_removed);
        assert!(!output_path.exists());
    }

    #[tokio::test]
    async fn measure_command_startup_timeout_is_operational_error_without_report() {
        let base = std::env::temp_dir().join(format!(
            "anchor-measurement-command-timeout-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let dataset_path = base.with_extension("ndjson");
        let report_path = base.with_extension("json");
        let error = measure_command(
            1,
            1,
            Some(dataset_path.clone()),
            Some(report_path.clone()),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .expect_err("startup timeout is an operational error");
        assert!(error.contains("startup timeout expired"));
        assert!(!dataset_path.exists());
        assert!(!report_path.exists());
    }

    #[test]
    fn command_outcomes_preserve_success_and_failed_gate_exit_codes() {
        assert_eq!(
            exit_code_for_outcome(CommandOutcome::Success),
            ExitCode::from(0)
        );
        assert_eq!(
            exit_code_for_outcome(CommandOutcome::MeasurementFailedGates),
            ExitCode::from(2)
        );
        // Operational errors remain the Err branch in main and therefore exit code 1.
        assert_eq!(ExitCode::FAILURE, ExitCode::from(1));
    }

    #[tokio::test]
    async fn interrupt_before_first_sample_cannot_produce_valid_measurement() {
        let output_path = std::env::temp_dir().join(format!(
            "anchor-measurement-pre-interrupt-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let mut runtime = try_start_recording_runtime(
            RecordingScenario::Stationary,
            Some(output_path.clone()),
            "127.0.0.1:0".parse().unwrap(),
            None,
        )
        .await
        .expect("runtime");
        let wait = wait_for_measurement(
            Duration::from_secs(1),
            Duration::from_secs(1),
            &runtime.shared_state,
            &runtime.recorder,
            &mut runtime.sample_rx,
            std::future::ready(()),
        )
        .await;
        assert_eq!(wait, MeasurementWaitOutcome::InterruptedBeforeFirstSample);
        assert!(finalize_recording(
            runtime.receiver,
            &runtime.shared_state,
            runtime.recorder,
            &output_path,
            false,
        )
        .await
        .is_err());
        assert!(!output_path.exists());
    }

    #[tokio::test]
    async fn interrupt_after_intervals_keeps_interrupted_dataset_for_failed_report() {
        let output_path = std::env::temp_dir().join(format!(
            "anchor-measurement-mid-interrupt-{}-{}.ndjson",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let mut runtime = try_start_recording_runtime(
            RecordingScenario::Stationary,
            Some(output_path.clone()),
            "127.0.0.1:0".parse().unwrap(),
            None,
        )
        .await
        .expect("runtime");
        let target = runtime.receiver.local_addr();
        tokio::spawn(async move {
            let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            for (sequence, elapsed) in [(42, 0), (43, 16_667), (44, 33_334)] {
                socket
                    .send_to(&measurement_payload(sequence, elapsed), target)
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let wait = wait_for_measurement(
            Duration::from_secs(1),
            Duration::from_secs(2),
            &runtime.shared_state,
            &runtime.recorder,
            &mut runtime.sample_rx,
            async { tokio::time::sleep(Duration::from_millis(80)).await },
        )
        .await;
        assert_eq!(wait, MeasurementWaitOutcome::InterruptedDuringWindow);
        let finalized = finalize_recording(
            runtime.receiver,
            &runtime.shared_state,
            runtime.recorder,
            &output_path,
            false,
        )
        .await
        .expect("non-empty interrupted dataset is preserved");
        let analysis = analyze_dataset_file(&output_path).expect("analyze");
        assert!(analysis.source_interval_us.count >= 1);
        let report = build_report(
            &analysis,
            &finalized.summary,
            2_000_000,
            60,
            ReceiverCounters::delta(&ReceiverMetrics::default(), &finalized.receiver_metrics),
        );
        assert!(!report.integrity_evaluation.passed);
        assert_eq!(
            report
                .integrity_evaluation
                .gates
                .iter()
                .find(|gate| gate.id == "recorder.completed_cleanly")
                .unwrap()
                .result,
            anchor_desktop_lib::transport_measurement::GateResult::Failed
        );
        let _ = fs::remove_file(output_path);
    }

    #[tokio::test]
    async fn measure_refuses_to_overwrite_existing_report_before_starting_receiver() {
        let report_path = std::env::temp_dir().join(format!(
            "anchor-existing-measurement-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        fs::write(&report_path, "preserve-me").expect("write existing report");
        let dataset_path = report_path.with_extension("ndjson");
        let err = run(vec![
            "measure".to_owned(),
            "--duration-seconds".to_owned(),
            "1".to_owned(),
            "--dataset-output".to_owned(),
            dataset_path.display().to_string(),
            "--report-output".to_owned(),
            report_path.display().to_string(),
        ])
        .await
        .expect_err("existing report must fail");
        assert!(err.contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&report_path).unwrap(), "preserve-me");
        assert!(!dataset_path.exists());
        let _ = fs::remove_file(report_path);
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
