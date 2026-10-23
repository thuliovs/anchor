use image::GenericImageView;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, Runtime,
};
use tokio::sync::watch;

pub mod calibration;
pub mod dataset;
pub mod latency_measurement;
pub mod latency_recorder;
pub mod latency_session;
pub mod live_motion;
pub mod measurement_integrity;
pub mod motion_filtering;
pub mod overlay;
pub mod protocol;
pub mod receiver;
pub mod transport_measurement;

use live_motion::runtime::{read_live_tilt_snapshot, LiveTiltRuntimeHandle, SharedLiveTiltRuntime};
use overlay::{OverlayMode, OverlaySnapshot, OverlayState, SharedOverlayState};
use receiver::{
    udp::{start_udp_receiver, UdpReceiverConfig, UdpReceiverHandle},
    ReceiverSnapshotDto, ReceiverState, SharedReceiverState,
};

type SharedReceiverHandle = Arc<Mutex<Option<UdpReceiverHandle>>>;
type SharedLiveTiltHandle = Arc<Mutex<Option<LiveTiltRuntimeHandle>>>;

const MOTION_SAMPLE_EVENT: &str = "anchor-motion-sample-v1";
const OVERLAY_BACKEND_OPEN_TIMEOUT: Duration = Duration::from_secs(8);

fn read_receiver_snapshot(
    receiver_state: &SharedReceiverState,
    now: Instant,
) -> Result<ReceiverSnapshotDto, String> {
    let state = receiver_state
        .lock()
        .map_err(|_| "receiver state is unavailable".to_owned())?;

    Ok(state.snapshot(now).to_dto(now))
}

#[tauri::command]
fn get_receiver_snapshot(
    receiver_state: tauri::State<'_, SharedReceiverState>,
) -> Result<ReceiverSnapshotDto, String> {
    read_receiver_snapshot(&receiver_state, Instant::now())
}

#[tauri::command]
fn get_live_tilt_snapshot(
    live_tilt_state: tauri::State<'_, SharedLiveTiltRuntime>,
) -> Result<live_motion::runtime::LiveTiltRuntimeSnapshotV1, String> {
    read_live_tilt_snapshot(&live_tilt_state, Instant::now())
}

#[tauri::command]
fn get_overlay_snapshot(
    overlay_state: tauri::State<'_, SharedOverlayState>,
) -> Result<OverlaySnapshot, String> {
    overlay::get_overlay_snapshot(&overlay_state)
}

#[tauri::command]
async fn open_overlay(
    app: AppHandle,
    overlay_state: tauri::State<'_, SharedOverlayState>,
) -> Result<OverlaySnapshot, String> {
    let shared_state = overlay_state.inner().clone();
    let plan = overlay::begin_open_overlay(&app, &shared_state)?;
    let Some(generation) = plan.generation else {
        return Ok(plan.snapshot);
    };

    let task_app = app.clone();
    let task_state = shared_state.clone();
    let task = tauri::async_runtime::spawn_blocking(move || {
        overlay::run_open_overlay_attempt(&task_app, &task_state, generation)
    });

    match tokio::time::timeout(OVERLAY_BACKEND_OPEN_TIMEOUT, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => overlay::fail_open_attempt(
            &app,
            &shared_state,
            generation,
            format!("overlay opening task failed: {error}"),
        ),
        Err(_) => overlay::fail_open_attempt(
            &app,
            &shared_state,
            generation,
            format!(
                "A criação do overlay não respondeu em {} segundos e foi invalidada.",
                OVERLAY_BACKEND_OPEN_TIMEOUT.as_secs()
            ),
        ),
    }
}

#[tauri::command]
fn set_overlay_mode(
    app: AppHandle,
    overlay_state: tauri::State<'_, SharedOverlayState>,
    mode: OverlayMode,
) -> Result<OverlaySnapshot, String> {
    overlay::set_overlay_mode(&app, &overlay_state, mode)
}

#[tauri::command]
fn close_overlay(
    app: AppHandle,
    overlay_state: tauri::State<'_, SharedOverlayState>,
) -> Result<OverlaySnapshot, String> {
    overlay::close_overlay(&app, &overlay_state)
}

fn spawn_motion_sample_bridge<R: Runtime>(
    app: AppHandle<R>,
    mut motion_sample_rx: watch::Receiver<Option<protocol::MotionSampleV1>>,
) {
    tauri::async_runtime::spawn(async move {
        while motion_sample_rx.changed().await.is_ok() {
            if let Some(sample) = motion_sample_rx.borrow().clone() {
                if let Err(err) = app.emit(MOTION_SAMPLE_EVENT, &sample) {
                    eprintln!("failed to emit {MOTION_SAMPLE_EVENT}: {err}");
                }
            }
        }
    });
}

fn create_tray_menu<R: Runtime>(app: &AppHandle<R>) -> Result<Menu<R>, tauri::Error> {
    let menu = Menu::new(app)?;

    let status_item = MenuItem::with_id(
        app,
        "status",
        "Status Conexão: Desconectado",
        false,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let show_main_item =
        MenuItem::with_id(app, "show-main", "Mostrar diagnóstico", true, None::<&str>)?;
    let configure_overlay_item = MenuItem::with_id(
        app,
        "overlay-configuration",
        "Recuperar overlay (configuração)",
        true,
        None::<&str>,
    )?;
    let close_overlay_item =
        MenuItem::with_id(app, "overlay-close", "Fechar overlay", true, None::<&str>)?;

    menu.append(&status_item)?;
    menu.append(&show_main_item)?;
    menu.append(&configure_overlay_item)?;
    menu.append(&close_overlay_item)?;
    menu.append(&quit_item)?;

    Ok(menu)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_receiver_snapshot,
            get_live_tilt_snapshot,
            get_overlay_snapshot,
            open_overlay,
            set_overlay_mode,
            close_overlay
        ])
        .setup(|app| {
            let receiver_state: SharedReceiverState =
                Arc::new(Mutex::new(ReceiverState::default()));
            let receiver_handle: SharedReceiverHandle = Arc::new(Mutex::new(None));
            let live_tilt_handle: SharedLiveTiltHandle = Arc::new(Mutex::new(None));
            let overlay_state: SharedOverlayState = Arc::new(Mutex::new(OverlayState::default()));
            let (motion_sample_tx, motion_sample_rx) = watch::channel(None);
            let live_tilt = live_motion::runtime::start_live_tilt_runtime_from_env();
            let live_tilt_state = live_tilt.state.clone();
            let live_tilt_sink = live_tilt.sink.clone();

            app.manage(receiver_state.clone());
            app.manage(receiver_handle.clone());
            app.manage(live_tilt_state);
            app.manage(live_tilt_handle.clone());
            app.manage(overlay_state);
            if let Ok(mut slot) = live_tilt_handle.lock() {
                *slot = Some(live_tilt);
            } else {
                eprintln!("failed to store live tilt runtime handle");
            }
            spawn_motion_sample_bridge(app.handle().clone(), motion_sample_rx);

            tauri::async_runtime::spawn(async move {
                match start_udp_receiver(
                    UdpReceiverConfig {
                        accepted_sample_sink: Some(live_tilt_sink),
                        ..UdpReceiverConfig::default()
                    },
                    receiver_state,
                    motion_sample_tx,
                )
                .await
                {
                    Ok(handle) => {
                        if let Ok(mut slot) = receiver_handle.lock() {
                            *slot = Some(handle);
                        } else {
                            eprintln!("failed to store motion receiver handle");
                        }
                    }
                    Err(err) => {
                        eprintln!(
                            "failed to start motion receiver on {}:{}: {}",
                            receiver::DEFAULT_RECEIVER_HOST,
                            receiver::DEFAULT_RECEIVER_PORT,
                            err
                        );
                    }
                }
            });

            let menu = create_tray_menu(app.handle())?;

            // Carrega o ícone do tray usando a biblioteca image
            let icon_bytes = include_bytes!("../icons/icon.png");
            let img = image::load_from_memory(icon_bytes)
                .map_err(|e| tauri::Error::AssetNotFound(format!("Failed to load icon: {}", e)))?;

            let (width, height) = img.dimensions();
            let rgba = img.to_rgba8().into_raw();

            let icon = tauri::image::Image::new_owned(rgba, width, height);

            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(icon)
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show-main" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "overlay-configuration" => {
                        let state = app.state::<SharedOverlayState>();
                        if let Err(error) =
                            overlay::set_overlay_mode(app, &state, OverlayMode::Configuration)
                        {
                            eprintln!("failed to recover overlay configuration mode: {error}");
                        }
                    }
                    "overlay-close" => {
                        let state = app.state::<SharedOverlayState>();
                        if let Err(error) = overlay::close_overlay(app, &state) {
                            eprintln!("failed to close overlay from tray: {error}");
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            if let Some(main_window) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                main_window.on_window_event(move |event| {
                    if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                        app_handle.exit(0);
                    }
                });
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let receiver = app
                .state::<SharedReceiverHandle>()
                .lock()
                .ok()
                .and_then(|mut slot| slot.take());
            let live_tilt = app
                .state::<SharedLiveTiltHandle>()
                .lock()
                .ok()
                .and_then(|mut slot| slot.take());

            tauri::async_runtime::block_on(async move {
                if let Some(receiver) = receiver {
                    receiver.shutdown().await;
                }
                if let Some(live_tilt) = live_tilt {
                    live_tilt.shutdown().await;
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::read_receiver_snapshot;
    use crate::receiver::{ReceiverState, SharedReceiverState};
    use std::{
        sync::{Arc, Mutex},
        time::Instant,
    };

    #[test]
    fn get_receiver_snapshot_command_does_not_mutate_state() {
        let now = Instant::now();
        let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));

        let before = shared_state
            .lock()
            .expect("shared state lock")
            .snapshot(now);
        let dto = read_receiver_snapshot(&shared_state, now).expect("snapshot dto");
        let after = shared_state
            .lock()
            .expect("shared state lock")
            .snapshot(now);

        assert_eq!(dto.status, before.to_dto(now).status);
        assert_eq!(before, after);
    }

    #[test]
    fn get_receiver_snapshot_command_returns_controlled_error_for_poisoned_mutex() {
        let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
        let poisoned_state = shared_state.clone();

        let _ = std::panic::catch_unwind(move || {
            let _guard = poisoned_state.lock().expect("shared state lock");
            panic!("poison receiver state");
        });

        let err = read_receiver_snapshot(&shared_state, Instant::now()).expect_err("poisoned lock");
        assert_eq!(err, "receiver state is unavailable");
    }
}
