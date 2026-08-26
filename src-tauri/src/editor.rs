use std::process::Command;

use crate::read_setting;

/// Open a folder/file path in the user's configured editor.
///
/// `command` overrides the global `editor_command` setting when provided
/// (e.g. a per-tree-source command). The command string may contain a
/// `{path}` placeholder; when absent, the path is appended as the last
/// argument. Examples: `code`, `code {path}`, `subl {path}`, `vim {path}`.
///
/// The command is run via `sh -c` so that shell scripts (e.g. the `code` CLI
/// wrapper) inherit a proper environment and PATH, and the editor process is
/// detached so it outlives the app.
#[tauri::command]
pub fn open_in_editor<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
    command: Option<String>,
) -> Result<(), String> {
    let cmd = command
        .filter(|c| !c.trim().is_empty())
        .or_else(|| read_setting(&app, "editor_command"))
        .unwrap_or_else(|| "code".to_string());
    let shell_cmd = build_shell_command(&cmd, &path);
    Command::new("sh")
        .arg("-c")
        .arg(&shell_cmd)
        .spawn()
        .map_err(|e| format!("Failed to launch editor: {e}"))?;
    Ok(())
}

/// Build a shell command string, substituting or appending the path.
/// The path is single-quoted to guard against spaces and shell metacharacters.
fn build_shell_command(cmd: &str, path: &str) -> String {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return format!("code '{}'", escape_single_quotes(path));
    }
    if trimmed.contains("{path}") {
        return trimmed.replace("{path}", &format!("'{}'", escape_single_quotes(path)));
    }
    format!("{} '{}'", trimmed, escape_single_quotes(path))
}

/// Escape single quotes for safe inclusion inside a single-quoted shell string.
fn escape_single_quotes(s: &str) -> String {
    s.replace('\'', "'\\''")
}
