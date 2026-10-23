use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

pub const OVERLAY_LABEL: &str = "overlay";
pub const OVERLAY_STATE_EVENT: &str = "anchor-overlay-state-v1";

pub type SharedOverlayState = Arc<Mutex<OverlayState>>;

pub struct OpenOverlayPlan {
    pub generation: Option<u64>,
    pub snapshot: OverlaySnapshot,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    #[default]
    Configuration,
    Operation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayLifecycle {
    #[default]
    Closed,
    Opening,
    Open,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlaySnapshot {
    pub supported: bool,
    pub platform: &'static str,
    pub lifecycle: OverlayLifecycle,
    pub is_open: bool,
    pub mode: OverlayMode,
    pub last_error: Option<String>,
}

#[derive(Debug, Default)]
pub struct OverlayState {
    lifecycle: OverlayLifecycle,
    mode: OverlayMode,
    last_error: Option<String>,
    open_generation: u64,
}

impl OverlayState {
    fn snapshot(&self) -> OverlaySnapshot {
        OverlaySnapshot {
            supported: cfg!(target_os = "windows"),
            platform: std::env::consts::OS,
            lifecycle: self.lifecycle,
            is_open: self.lifecycle == OverlayLifecycle::Open,
            mode: self.mode,
            last_error: self.last_error.clone(),
        }
    }

    fn opened(&mut self, mode: OverlayMode) {
        self.lifecycle = OverlayLifecycle::Open;
        self.mode = mode;
        self.last_error = None;
    }

    fn closed(&mut self) {
        self.lifecycle = OverlayLifecycle::Closed;
        self.mode = OverlayMode::Configuration;
        self.last_error = None;
        self.open_generation = self.open_generation.wrapping_add(1);
    }

    fn failed(&mut self, message: String) {
        self.last_error = Some(message);
    }

    fn begin_open(&mut self) -> Option<u64> {
        if self.lifecycle == OverlayLifecycle::Opening {
            return None;
        }

        self.open_generation = self.open_generation.wrapping_add(1);
        self.lifecycle = OverlayLifecycle::Opening;
        self.mode = OverlayMode::Configuration;
        self.last_error = None;
        Some(self.open_generation)
    }

    fn is_current_open(&self, generation: u64) -> bool {
        self.lifecycle == OverlayLifecycle::Opening && self.open_generation == generation
    }
}

#[derive(Clone, Copy, Debug)]
struct WorkArea {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn read_snapshot(state: &SharedOverlayState) -> Result<OverlaySnapshot, String> {
    state
        .lock()
        .map(|state| state.snapshot())
        .map_err(|_| "overlay state is unavailable".to_owned())
}

fn update_state(
    state: &SharedOverlayState,
    update: impl FnOnce(&mut OverlayState),
) -> Result<OverlaySnapshot, String> {
    let mut state = state
        .lock()
        .map_err(|_| "overlay state is unavailable".to_owned())?;
    update(&mut state);
    Ok(state.snapshot())
}

fn publish<R: Runtime>(app: &AppHandle<R>, snapshot: &OverlaySnapshot) {
    if let Err(error) = app.emit(OVERLAY_STATE_EVENT, snapshot) {
        eprintln!("failed to emit {OVERLAY_STATE_EVENT}: {error}");
    }
}

fn record_failure<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedOverlayState,
    message: String,
) -> Result<OverlaySnapshot, String> {
    let snapshot = update_state(state, |state| state.failed(message))?;
    publish(app, &snapshot);
    Ok(snapshot)
}

fn record_closed_failure<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedOverlayState,
    message: String,
) -> Result<OverlaySnapshot, String> {
    let snapshot = update_state(state, |state| {
        state.closed();
        state.failed(message);
    })?;
    publish(app, &snapshot);
    Ok(snapshot)
}

fn finish_open_failure<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedOverlayState,
    generation: u64,
    message: String,
) -> Result<OverlaySnapshot, String> {
    let snapshot = update_state(state, |state| {
        if state.is_current_open(generation) {
            state.closed();
            state.failed(message);
        }
    })?;
    publish(app, &snapshot);
    Ok(snapshot)
}

pub fn fail_open_attempt<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedOverlayState,
    generation: u64,
    message: String,
) -> Result<OverlaySnapshot, String> {
    finish_open_failure(app, state, generation, message)
}

#[cfg(target_os = "windows")]
fn primary_work_area() -> Result<WorkArea, String> {
    use std::ffi::c_void;
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA},
    };

    let mut rect = RECT::default();
    // SAFETY: SPI_GETWORKAREA writes a RECT to the valid pointer supplied here.
    let succeeded = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            (&mut rect as *mut RECT).cast::<c_void>(),
            0,
        )
    };
    if succeeded == 0 {
        return Err("Windows did not provide the primary display work area".to_owned());
    }

    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return Err("Windows returned an invalid primary display work area".to_owned());
    }

    Ok(WorkArea {
        x: rect.left,
        y: rect.top,
        width: width as u32,
        height: height as u32,
    })
}

#[cfg(not(target_os = "windows"))]
fn primary_work_area() -> Result<WorkArea, String> {
    Err(format!(
        "the experimental overlay is disabled on {}; C1 supports Windows only",
        std::env::consts::OS
    ))
}

fn configure_geometry<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    let area = primary_work_area()?;
    window
        .set_position(PhysicalPosition::new(area.x, area.y))
        .map_err(|error| format!("failed to position overlay: {error}"))?;
    window
        .set_size(PhysicalSize::new(area.width, area.height))
        .map_err(|error| format!("failed to size overlay: {error}"))?;
    Ok(())
}

fn build_overlay<R: Runtime>(
    app: &AppHandle<R>,
    state: SharedOverlayState,
) -> Result<WebviewWindow<R>, String> {
    let window = WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        WebviewUrl::App("index.html?window=overlay".into()),
    )
    .title("Anchor - Overlay experimental C1")
    .transparent(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .visible(false)
    .build()
    .map_err(|error| format!("failed to create overlay window: {error}"))?;

    let event_app = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            match update_state(&state, OverlayState::closed) {
                Ok(snapshot) => publish(&event_app, &snapshot),
                Err(error) => eprintln!("failed to mark destroyed overlay as closed: {error}"),
            }
        }
    });
    Ok(window)
}

pub fn get_overlay_snapshot(state: &SharedOverlayState) -> Result<OverlaySnapshot, String> {
    read_snapshot(state)
}

pub fn begin_open_overlay<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedOverlayState,
) -> Result<OpenOverlayPlan, String> {
    if !cfg!(target_os = "windows") {
        let snapshot = record_failure(
            app,
            state,
            format!(
                "Overlay C1 indisponível em {}; suporte validável apenas no Windows.",
                std::env::consts::OS
            ),
        )?;
        return Ok(OpenOverlayPlan {
            generation: None,
            snapshot,
        });
    }

    let (generation, opening_snapshot) = {
        let mut state_guard = state
            .lock()
            .map_err(|_| "overlay state is unavailable".to_owned())?;
        let generation = state_guard.begin_open();
        (generation, state_guard.snapshot())
    };
    publish(app, &opening_snapshot);

    Ok(OpenOverlayPlan {
        generation,
        snapshot: opening_snapshot,
    })
}

pub fn run_open_overlay_attempt<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedOverlayState,
    generation: u64,
) -> Result<OverlaySnapshot, String> {
    let existing = app.get_webview_window(OVERLAY_LABEL);
    let window = match existing {
        Some(window) => window,
        None => match build_overlay(app, state.clone()) {
            Ok(window) => window,
            Err(error) => return finish_open_failure(app, state, generation, error),
        },
    };

    let configured = configure_geometry(&window)
        .and_then(|()| {
            window
                .set_ignore_cursor_events(false)
                .map_err(|error| format!("failed to enable overlay input: {error}"))
        })
        .and_then(|()| {
            window
                .show()
                .map_err(|error| format!("failed to show overlay: {error}"))
        });

    if let Err(error) = configured {
        let _ = window.hide();
        return finish_open_failure(app, state, generation, error);
    }

    let (snapshot, opened) = {
        let mut state_guard = state
            .lock()
            .map_err(|_| "overlay state is unavailable".to_owned())?;
        let opened = state_guard.is_current_open(generation);
        if opened {
            state_guard.opened(OverlayMode::Configuration);
        }
        (state_guard.snapshot(), opened)
    };
    if !opened {
        // A close/recovery action cancelled this generation while WebView2 was being created.
        let _ = window.hide();
    }
    publish(app, &snapshot);
    Ok(snapshot)
}

pub fn set_overlay_mode<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedOverlayState,
    mode: OverlayMode,
) -> Result<OverlaySnapshot, String> {
    let current = read_snapshot(state)?;
    if !current.supported {
        return record_failure(
            app,
            state,
            format!("Overlay C1 indisponível em {}.", current.platform),
        );
    }
    if current.lifecycle != OverlayLifecycle::Open {
        return record_failure(app, state, "O overlay não está aberto.".to_owned());
    }

    let Some(window) = app.get_webview_window(OVERLAY_LABEL) else {
        let snapshot = update_state(state, |state| {
            state.closed();
            state.failed("A janela do overlay não existe mais.".to_owned());
        })?;
        publish(app, &snapshot);
        return Ok(snapshot);
    };

    let ignore_input = mode == OverlayMode::Operation;
    if let Err(error) = window.set_ignore_cursor_events(ignore_input) {
        // If click-through cannot be changed reliably, hide the window so it cannot trap input.
        let _ = window.hide();
        let snapshot = update_state(state, |state| {
            state.closed();
            state.failed(format!("failed to change overlay input mode: {error}"));
        })?;
        publish(app, &snapshot);
        return Ok(snapshot);
    }

    if mode == OverlayMode::Operation {
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.set_focus();
        }
    }

    let snapshot = update_state(state, |state| state.opened(mode))?;
    publish(app, &snapshot);
    Ok(snapshot)
}

pub fn close_overlay<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedOverlayState,
) -> Result<OverlaySnapshot, String> {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        // Disable click-through before hiding; a later open always starts in configuration mode.
        if let Err(error) = window.set_ignore_cursor_events(false) {
            let _ = window.hide();
            return record_closed_failure(
                app,
                state,
                format!("overlay hidden, but input mode reset failed: {error}"),
            );
        }
        if let Err(error) = window.hide() {
            return record_failure(app, state, format!("failed to hide overlay: {error}"));
        }
    }

    let snapshot = update_state(state, OverlayState::closed)?;
    publish(app, &snapshot);
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::{OverlayLifecycle, OverlayMode, OverlayState};

    #[test]
    fn overlay_state_opens_in_configuration_and_switches_mode() {
        let mut state = OverlayState::default();
        let generation = state.begin_open().expect("first opening generation");
        assert_eq!(state.snapshot().lifecycle, OverlayLifecycle::Opening);
        assert!(!state.snapshot().is_open);
        assert!(state.is_current_open(generation));

        state.opened(OverlayMode::Configuration);
        assert!(state.snapshot().is_open);
        assert_eq!(state.snapshot().mode, OverlayMode::Configuration);

        state.opened(OverlayMode::Operation);
        assert_eq!(state.snapshot().mode, OverlayMode::Operation);
        assert_eq!(state.snapshot().last_error, None);
    }

    #[test]
    fn closing_resets_safe_configuration_mode() {
        let mut state = OverlayState::default();
        state.opened(OverlayMode::Operation);
        state.closed();

        assert!(!state.snapshot().is_open);
        assert_eq!(state.snapshot().mode, OverlayMode::Configuration);
    }

    #[test]
    fn close_cancels_an_opening_generation() {
        let mut state = OverlayState::default();
        let generation = state.begin_open().expect("opening generation");
        state.closed();

        assert!(!state.is_current_open(generation));
        assert_eq!(state.snapshot().lifecycle, OverlayLifecycle::Closed);
        assert!(state.begin_open().is_some());
    }

    #[test]
    fn concurrent_open_does_not_start_a_second_generation() {
        let mut state = OverlayState::default();
        let generation = state.begin_open().expect("opening generation");

        assert_eq!(state.begin_open(), None);
        assert!(state.is_current_open(generation));
    }

    #[test]
    fn controlled_failure_preserves_observable_state() {
        let mut state = OverlayState::default();
        state.failed("expected failure".to_owned());

        assert!(!state.snapshot().is_open);
        assert_eq!(
            state.snapshot().last_error.as_deref(),
            Some("expected failure")
        );
    }
}
