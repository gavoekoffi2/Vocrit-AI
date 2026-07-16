use crate::settings;
use crate::transcription_coordinator::is_transcribe_binding;
use crate::utils::cancel_current_operation;
use crate::TranscriptionCoordinator;
use log::{debug, warn};
use tauri::{AppHandle, Emitter, Manager};

/// Called by every shortcut backend (tauri-plugin-global-shortcut and handy-keys)
/// whenever a registered hotkey transitions between pressed / released.
///
/// This is the single bridge between OS-level key events and the transcription
/// pipeline. It MUST forward the event to the `TranscriptionCoordinator` for
/// transcribe bindings — otherwise pressing the shortcut does nothing at all.
pub fn handle_shortcut_event(app: &AppHandle, binding_id: &str, shortcut: &str, is_pressed: bool) {
    debug!(
        "Shortcut event: binding='{}' shortcut='{}' pressed={}",
        binding_id, shortcut, is_pressed
    );

    // Mirror the event to the frontend so debug/UI surfaces can react
    // (recording overlay, activity indicators, etc.).
    let _ = app.emit(
        "shortcut-event",
        serde_json::json!({
            "binding_id": binding_id,
            "shortcut": shortcut,
            "is_pressed": is_pressed,
        }),
    );

    // Cancel shortcut goes through the dedicated cancellation path that
    // tears down recording, overlay, tray state and notifies the coordinator.
    if binding_id == "cancel" {
        if is_pressed {
            cancel_current_operation(app);
        }
        return;
    }

    // For transcribe bindings we always forward the event (press AND release)
    // so push-to-talk can observe the release and stop recording automatically.
    if is_transcribe_binding(binding_id) {
        let push_to_talk = settings::get_settings(app).push_to_talk;
        if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
            coordinator.send_input(binding_id, shortcut, is_pressed, push_to_talk);
        } else {
            warn!(
                "Shortcut '{}' pressed but TranscriptionCoordinator is not initialized yet",
                binding_id
            );
        }
    }
}
