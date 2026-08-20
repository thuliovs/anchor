use image::GenericImageView;
use std::{
    process,
    sync::{Arc, Mutex},
    time::Instant,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, Runtime,
};
use tokio::sync::watch;

pub mod protocol;
pub mod receiver;

use receiver::{
    udp::{start_udp_receiver, UdpReceiverConfig, UdpReceiverHandle},
    ReceiverSnapshotDto, ReceiverState, SharedReceiverState,
};

type SharedReceiverHandle = Arc<Mutex<Option<UdpReceiverHandle>>>;

const MOTION_SAMPLE_EVENT: &str = "anchor-motion-sample-v1";

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

    menu.append(&status_item)?;
    menu.append(&quit_item)?;

    Ok(menu)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_receiver_snapshot])
        .setup(|app| {
            let receiver_state: SharedReceiverState =
                Arc::new(Mutex::new(ReceiverState::default()));
            let receiver_handle: SharedReceiverHandle = Arc::new(Mutex::new(None));
            let (motion_sample_tx, motion_sample_rx) = watch::channel(None);

            app.manage(receiver_state.clone());
            app.manage(receiver_handle.clone());
            spawn_motion_sample_bridge(app.handle().clone(), motion_sample_rx);

            tauri::async_runtime::spawn(async move {
                match start_udp_receiver(
                    UdpReceiverConfig::default(),
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
                .on_menu_event(|_app, event| {
                    if event.id.as_ref() == "quit" {
                        process::exit(0);
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
