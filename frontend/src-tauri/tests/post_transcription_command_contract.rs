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
