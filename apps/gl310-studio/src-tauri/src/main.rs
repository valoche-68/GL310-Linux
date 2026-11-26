//! Tauri v2 desktop backend for GL310 Studio.
//! Calls `gl310-core` directly in memory and emits live telemetry / VU-meter / log updates over IPC.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, RunEvent, State, WindowEvent};

use gl310_core::firmware::{
    fetch_and_install_official_firmware, import_firmware_from_path, FirmwareStatus,
};
use gl310_core::recorder::{RecordingStatus, RecordingSummary};
use gl310_core::settings::{AudioInput, LedMode, Settings};
use gl310_core::{run_doctor, CaptureEngine, DoctorReport, EnginePhase, EngineSnapshot, LogEntry};

struct AppState {
    engine: Arc<CaptureEngine>,
}

#[derive(Clone, Serialize)]
struct FirmwareImportResult {
    ok: bool,
    message: String,
    status: FirmwareStatus,
}

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> EngineSnapshot {
    state.engine.snapshot()
}

#[tauri::command]
fn get_logs(state: State<'_, AppState>) -> Vec<LogEntry> {
    state.engine.logs()
}

#[tauri::command]
fn clear_logs(state: State<'_, AppState>) {
    state.engine.clear_logs();
}

#[tauri::command]
async fn get_doctor(state: State<'_, AppState>) -> Result<DoctorReport, String> {
    let fw_ov = state.engine.snapshot().settings.firmware_dir_override;
    tauri::async_runtime::spawn_blocking(move || {
        run_doctor(fw_ov.as_deref().map(std::path::Path::new))
    })
    .await
    .map_err(|e| format!("Doctor task failed: {e}"))
}

#[tauri::command]
async fn start_capture(state: State<'_, AppState>) -> Result<EngineSnapshot, String> {
    let engine = Arc::clone(&state.engine);
    tauri::async_runtime::spawn_blocking(move || engine.start_capture_sync())
        .await
        .map_err(|e| format!("Start capture task failed: {e}"))?
}

#[tauri::command]
async fn stop_capture(state: State<'_, AppState>) -> Result<EngineSnapshot, String> {
    let engine = Arc::clone(&state.engine);
    tauri::async_runtime::spawn_blocking(move || engine.stop_capture_sync())
        .await
        .map_err(|e| format!("Stop capture task failed: {e}"))?
}

#[tauri::command]
fn update_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    state.engine.update_settings(settings)
}

#[tauri::command]
fn set_led(state: State<'_, AppState>, mode: LedMode) -> Result<(), String> {
    state.engine.set_led(mode)
}

#[tauri::command]
fn set_audio_input(state: State<'_, AppState>, input: AudioInput) -> Result<(), String> {
    state.engine.set_audio_input(input)
}

#[tauri::command]
fn set_analog_volume(state: State<'_, AppState>, volume: u8) -> Result<(), String> {
    state.engine.set_analog_volume(volume)
}

#[tauri::command]
fn set_encoder_volume(state: State<'_, AppState>, volume: u8) -> Result<(), String> {
    state.engine.set_encoder_volume(volume)
}

#[tauri::command]
fn set_pc_monitor(state: State<'_, AppState>, enabled: bool, muted: bool, volume: u8) {
    state.engine.set_pc_monitor(enabled, muted, volume);
}

#[tauri::command]
fn start_recording(state: State<'_, AppState>) -> Result<RecordingStatus, String> {
    state.engine.start_recording(None)
}

#[tauri::command]
async fn stop_recording(state: State<'_, AppState>) -> Result<RecordingSummary, String> {
    let engine = Arc::clone(&state.engine);
    tauri::async_runtime::spawn_blocking(move || engine.stop_recording())
        .await
        .map_err(|e| format!("Stop recording task failed: {e}"))?
}

#[tauri::command]
fn take_snapshot(state: State<'_, AppState>) -> Result<String, String> {
    state.engine.take_snapshot()
}

#[tauri::command]
fn refresh_device(state: State<'_, AppState>) {
    state.engine.refresh_probe();
}

#[tauri::command]
async fn firmware_fetch(state: State<'_, AppState>) -> Result<FirmwareImportResult, String> {
    let engine = Arc::clone(&state.engine);
    engine.push_log(
        "info",
        "Downloading & auditing official AVerMedia v3.2802.64.40 firmware archive...",
    );
    tauri::async_runtime::spawn_blocking(move || match fetch_and_install_official_firmware(None) {
        Ok((_fw, log)) => {
            engine.push_log(
                "info",
                "Official firmware verified (SHA-256 + MD5) and installed.",
            );
            let st = engine.refresh_firmware_status();
            engine.refresh_probe();
            Ok(FirmwareImportResult {
                ok: true,
                message: log,
                status: st,
            })
        }
        Err(e) => {
            engine.push_log("error", format!("Firmware fetch failed: {e}"));
            Err(e)
        }
    })
    .await
    .map_err(|e| format!("Firmware fetch task failed: {e}"))?
}

#[tauri::command]
async fn firmware_import(
    state: State<'_, AppState>,
    path: String,
) -> Result<FirmwareImportResult, String> {
    let trimmed = path.trim().to_string();
    if trimmed.is_empty() {
        return Err("Please provide a path to a Windows driver ZIP or folder".into());
    }
    let engine = Arc::clone(&state.engine);
    tauri::async_runtime::spawn_blocking(move || {
        let src = PathBuf::from(&trimmed);
        engine.push_log(
            "info",
            format!("Importing GL310 firmware from {}...", src.display()),
        );
        match import_firmware_from_path(&src, None) {
            Ok(fw) => {
                let msg = format!(
                    "SHA-256 verified and installed into {}",
                    fw.directory.display()
                );
                engine.push_log("info", msg.clone());
                let st = engine.refresh_firmware_status();
                engine.refresh_probe();
                Ok(FirmwareImportResult {
                    ok: true,
                    message: msg,
                    status: st,
                })
            }
            Err(e) => {
                engine.push_log("error", format!("Firmware import failed: {e}"));
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| format!("Firmware import task failed: {e}"))?
}

fn toggle_main_window(app: &tauri::AppHandle, engine: &CaptureEngine) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.unminimize();
            let _ = win.set_focus();
            let snap = engine.snapshot();
            let _ = app.emit("gl310://snapshot", &snap);
        }
    }
}

fn main() {
    let engine = Arc::new(CaptureEngine::start(None));
    let engine_emitter = Arc::clone(&engine);
    let engine_tray_menu = Arc::clone(&engine);
    let engine_tray_click = Arc::clone(&engine);
    let engine_win = Arc::clone(&engine);
    let engine_exit = Arc::clone(&engine);

    let emitter_stop = Arc::new(AtomicBool::new(false));
    let emitter_stop_bg = Arc::clone(&emitter_stop);
    let emitter_handle: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    let emitter_handle_setup = Arc::clone(&emitter_handle);

    let tray_active = Arc::new(AtomicBool::new(false));
    let tray_active_setup = Arc::clone(&tray_active);
    let tray_active_win = Arc::clone(&tray_active);
    let quitting = Arc::new(AtomicBool::new(false));
    let quitting_tray = Arc::clone(&quitting);
    let quitting_win = Arc::clone(&quitting);

    let app = tauri::Builder::default()
        .manage(AppState { engine })
        .setup(move |app| {
            let app_handle = app.handle().clone();
            // Emit live status & VU telemetry ONLY when the window is visible and state actually changed.
            // Keeps idle WebKitGTK CPU near 0% and eliminates 8 Hz IPC/repaint storms.
            let handle = thread::spawn(move || {
                let mut last_payload = String::new();
                while !emitter_stop_bg.load(Ordering::Relaxed) {
                    let visible = app_handle
                        .get_webview_window("main")
                        .and_then(|w| w.is_visible().ok())
                        .unwrap_or(true);
                    if !visible {
                        thread::sleep(Duration::from_millis(400));
                        continue;
                    }
                    let snap = engine_emitter.snapshot();
                    let phase = snap.phase;
                    if let Ok(serialized) = serde_json::to_string(&snap) {
                        if serialized != last_payload {
                            last_payload = serialized;
                            let _ = app_handle.emit("gl310://snapshot", &snap);
                        }
                    }
                    let sleep_ms = match phase {
                        EnginePhase::Streaming => 150,
                        EnginePhase::Booting => 200,
                        _ => 350,
                    };
                    thread::sleep(Duration::from_millis(sleep_ms));
                }
            });
            *emitter_handle_setup.lock().unwrap() = Some(handle);

            // System tray icon (bottom-right notification area on KDE / GNOME / Linux)
            // Allows closing the window to background while keeping capture/OBS active,
            // and quitting cleanly via right-click -> "Quitter GL310 Studio".
            let build_tray = (|| -> tauri::Result<()> {
                let item_toggle = MenuItem::with_id(
                    app,
                    "toggle",
                    "Afficher / Masquer GL310 Studio",
                    true,
                    None::<&str>,
                )?;
                let sep1 = PredefinedMenuItem::separator(app)?;
                let item_start =
                    MenuItem::with_id(app, "start", "Démarrer la capture", true, None::<&str>)?;
                let item_stop =
                    MenuItem::with_id(app, "stop", "Arrêter la capture", true, None::<&str>)?;
                let sep2 = PredefinedMenuItem::separator(app)?;
                let item_quit =
                    MenuItem::with_id(app, "quit", "Quitter GL310 Studio", true, None::<&str>)?;

                let menu = Menu::with_items(
                    app,
                    &[&item_toggle, &sep1, &item_start, &item_stop, &sep2, &item_quit],
                )?;

                let tray_icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))
                    .ok()
                    .or_else(|| app.default_window_icon().cloned());

                let mut builder = TrayIconBuilder::with_id("gl310-tray")
                    .tooltip("GL310 Studio — AVerMedia LGP Lite")
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(move |app_h, event| match event.id.as_ref() {
                        "toggle" => toggle_main_window(app_h, &engine_tray_menu),
                        "start" => {
                            let eng = Arc::clone(&engine_tray_menu);
                            let ah = app_h.clone();
                            thread::spawn(move || match eng.start_capture_sync() {
                                Ok(snap) => {
                                    let _ = ah.emit("gl310://snapshot", &snap);
                                }
                                Err(e) => {
                                    eng.push_log("warn", format!("Tray start failed: {e}"));
                                    let _ = ah.emit("gl310://snapshot", &eng.snapshot());
                                }
                            });
                        }
                        "stop" => {
                            let eng = Arc::clone(&engine_tray_menu);
                            let ah = app_h.clone();
                            thread::spawn(move || {
                                let _ = eng.stop_capture_sync();
                                let _ = ah.emit("gl310://snapshot", &eng.snapshot());
                            });
                        }
                        "quit" => {
                            quitting_tray.store(true, Ordering::SeqCst);
                            app_h.exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(move |tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            toggle_main_window(tray.app_handle(), &engine_tray_click);
                        }
                    });

                if let Some(icon) = tray_icon {
                    builder = builder.icon(icon);
                }

                builder.build(app)?;
                Ok(())
            })();

            match build_tray {
                Ok(()) => {
                    tray_active_setup.store(true, Ordering::SeqCst);
                }
                Err(e) => {
                    eprintln!("[gl310-studio] System tray unavailable ({e}) — close button will exit app");
                }
            }

            let _ = app.get_webview_window("main");
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if !quitting_win.load(Ordering::SeqCst) && tray_active_win.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                    engine_win.push_log(
                        "info",
                        "GL310 Studio réduit dans la barre système (clic droit sur l'icône → Quitter pour fermer).",
                    );
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_logs,
            clear_logs,
            get_doctor,
            start_capture,
            stop_capture,
            update_settings,
            set_led,
            set_audio_input,
            set_analog_volume,
            set_encoder_volume,
            set_pc_monitor,
            start_recording,
            stop_recording,
            take_snapshot,
            refresh_device,
            firmware_fetch,
            firmware_import,
        ])
        .build(tauri::generate_context!())
        .expect("error while building GL310 Studio");

    app.run(move |_app_handle, event| {
        if let RunEvent::Exit = event {
            // Stop telemetry thread and deterministically stop hardware encoder, remux MP4,
            // kill ffmpeg/pw-cat, unload PipeWire modules, and remove /dev/video14.
            emitter_stop.store(true, Ordering::SeqCst);
            if let Some(h) = emitter_handle.lock().unwrap().take() {
                let _ = h.join();
            }
            engine_exit.shutdown();
        }
    });
}
