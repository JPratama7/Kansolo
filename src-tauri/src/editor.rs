use std::process::Command;

use crate::read_setting;

/// Open a folder/file path in the user's configured editor.
///
/// `command` overrides the global `editor_command` setting when provided
/// (e.g. a per-tree-source command). The command string may contain a
/// `{path}` placeholder; when absent, the path is appended as the last
/// argument. Examples: `code`, `code {path}`, `subl {path}`, `vim {path}`.
///
/// Tokenize the command on whitespace and spawn it directly (no `sh -c`),
/// so shell metacharacters (`;`, `|`, `$()`, backticks) in the command string
/// are passed literally to the editor and never interpreted. `Command::new`
/// resolves the program via `PATH` on Unix, so the `code` CLI wrapper works
/// without a shell. The editor process is detached so it outlives the app.
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
    let (program, args) = parse_editor_command(&cmd, &path);
    Command::new(&program)
        .args(&args)
        .spawn()
        .map_err(|e| format!("Failed to launch editor: {e}"))?;
    Ok(())
}

/// Tokenize an editor command string into `(program, args)`, substituting
/// `{path}` with the literal path. Whitespace splitting only — no shell
/// quoting/escaping is honored, which is intentional: the command is a
/// config value, not a shell snippet, and treating it as raw tokens stops
/// `;`, `|`, and `$()` from being interpreted.
fn parse_editor_command(cmd: &str, path: &str) -> (String, Vec<String>) {
    let trimmed = cmd.trim();
    let mut tokens: Vec<String> = trimmed
        .split_whitespace()
        .map(|tok| {
            if tok == "{path}" {
                path.to_string()
            } else {
                tok.to_string()
            }
        })
        .collect();
    if tokens.is_empty() {
        tokens.push("code".to_string());
    }
    if !cmd.contains("{path}") {
        tokens.push(path.to_string());
    }
    let program = tokens.remove(0);
    (program, tokens)
}

#[cfg(test)]
mod tests {
    use super::parse_editor_command;

    #[test]
    fn bare_command_appends_path() {
        let (prog, args) = parse_editor_command("code", "/repo");
        assert_eq!(prog, "code");
        assert_eq!(args, vec!["/repo".to_string()]);
    }

    #[test]
    fn placeholder_substitutes_path() {
        let (prog, args) = parse_editor_command("code {path}", "/repo");
        assert_eq!(prog, "code");
        assert_eq!(args, vec!["/repo".to_string()]);
    }

    #[test]
    fn empty_command_defaults_to_code() {
        let (prog, args) = parse_editor_command("   ", "/repo");
        assert_eq!(prog, "code");
        assert_eq!(args, vec!["/repo".to_string()]);
    }

    #[test]
    fn shell_metacharacters_passed_literally_not_interpreted() {
        // ponytail: no sh -c, so `;` is a literal arg to the editor, not a
        // command separator. The whole string is one token (no whitespace), so
        // it becomes the program name — which simply fails to spawn with
        // "no such file" rather than executing `rm -rf ~`.
        let (prog, args) = parse_editor_command("code; rm -rf ~", "/repo");
        assert_eq!(prog, "code;");
        assert_eq!(
            args,
            vec![
                "rm".to_string(),
                "-rf".to_string(),
                "~".to_string(),
                "/repo".to_string()
            ]
        );
    }

    #[test]
    fn pipe_in_command_is_literal_token() {
        let (prog, args) = parse_editor_command("code | cat", "/repo");
        assert_eq!(prog, "code");
        assert_eq!(
            args,
            vec!["|".to_string(), "cat".to_string(), "/repo".to_string()]
        );
    }
}
