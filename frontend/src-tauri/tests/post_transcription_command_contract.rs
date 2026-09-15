#[test]
fn native_import_command_keeps_the_fixed_ipc_and_existing_job_contract() {
    let commands = include_str!("../src/audio/post_transcription/commands.rs");
    let start = commands
        .find("pub async fn gigastt_import_audio")
        .expect("native GigaSTT import command must exist");
    let tail = &commands[start..];
    let end = tail[1..]
        .find("#[tauri::command]")
        .map(|index| index + 1)
        .unwrap_or(tail.len());
    let command = &tail[..end];

    assert!(command.contains("source_path: String"));
    assert!(command.contains("title: String"));
    assert!(command.contains("Result<JobSnapshot, String>"));
    assert!(!command.contains("language:"));
    assert!(!command.contains("provider:"));
    assert!(!command.contains("model:"));
    assert!(command.contains("create_import_archive"));
    assert!(command.contains("persist_imported_meeting"));
    assert!(command.contains("launch_meeting"));
}

#[test]
fn native_import_command_is_registered_in_the_tauri_handler() {
    let lib = include_str!("../src/lib.rs");
    assert!(lib.contains("audio::post_transcription::commands::gigastt_import_audio,"));
}

#[test]
fn native_import_preaccept_work_is_owned_and_joined_on_app_close() {
    let commands = include_str!("../src/audio/post_transcription/commands.rs");
    let start = commands
        .find("pub async fn gigastt_import_audio")
        .expect("native GigaSTT import command must exist");
    let command = &commands[start
        ..commands[start..]
            .find("async fn launch_meeting")
            .map(|index| start + index)
            .unwrap_or(commands.len())];

    assert!(command.contains(".spawn_owned"));
    assert!(command.contains("validation_lease"));
    assert!(command.contains("cancellation_token()"));
    assert!(command.matches("check_active()").count() >= 3);
    assert!(commands.contains("self.imports.close().await"));
}

#[test]
fn every_tauri_launch_snapshots_current_vad_before_ownership_or_spawning() {
    let commands = include_str!("../src/audio/post_transcription/commands.rs");
    let start = commands
        .find("async fn launch_meeting")
        .expect("central GigaSTT launch function must exist");
    let launch = &commands[start
        ..commands[start..]
            .find("async fn acquire_job_ownership")
            .map(|index| start + index)
            .unwrap_or(commands.len())];

    let settings_load = launch
        .find("super::settings::load(&app)?")
        .expect("central launch must surface the persisted settings load");
    let ownership = launch
        .find("acquire_job_ownership(&app)")
        .expect("central launch must acquire transcript ownership");
    let spawn = launch
        .find("tokio::spawn")
        .expect("central launch must spawn the owned worker");

    assert!(
        settings_load < ownership,
        "settings errors must not acquire new ownership"
    );
    assert!(ownership < spawn);
    assert!(launch.contains("vad_enabled: settings.vad_enabled"));
    assert!(launch.contains("run_id: run_id.clone()"));

    for caller in [
        "gigastt_finalize_saved_meeting",
        "gigastt_transcribe_meeting",
        "run_owned_import",
    ] {
        let caller_start = commands.find(caller).expect("launch caller must exist");
        let caller_tail = &commands[caller_start..start];
        assert!(
            caller_tail.contains("launch_meeting("),
            "{caller} must use the central settings snapshot path"
        );
    }
}

#[test]
fn settings_load_and_save_share_one_recoverable_serialization_gate() {
    let settings = include_str!("../src/audio/post_transcription/settings.rs");

    assert!(settings.contains("static SETTINGS_ACCESS: Mutex<()> = Mutex::new(())"));
    assert!(settings.contains("unwrap_or_else(|poisoned| poisoned.into_inner())"));

    let load_start = settings
        .find("pub fn load")
        .expect("settings load must exist");
    let persist_start = settings
        .find("fn persist")
        .expect("settings persistence boundary must exist");
    let command_start = settings
        .find("pub async fn gigastt_save_settings")
        .expect("settings command must exist");
    let load = &settings[load_start..persist_start];
    let persist = &settings[persist_start..command_start];

    assert!(load.contains("lock_settings_access()"));
    assert!(persist.contains("lock_settings_access()"));
    assert!(persist.contains("save_with_rollback"));
}
