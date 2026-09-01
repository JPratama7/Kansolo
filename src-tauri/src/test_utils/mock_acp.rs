//! Minimal ACP agent mock for runner end-to-end tests.
//!
//! Runner tests spawn the lib test binary itself (`std::env::current_exe()`)
//! with `MOCK_ACP_BINARY=1` and `--exact mock_acp_server`. The mock then runs
//! the JSON-RPC loop on stdio: initialize, session/new, session/prompt,
//! session/cancel. Env knobs: `MOCK_ACP_UPDATES` (chunks per prompt),
//! `MOCK_ACP_HANG` (never answer, for cancel), `MOCK_ACP_PID_FILE`.

use std::io::{BufRead, Write};

/// Run the mock agent server loop on stdin/stdout until EOF or cancel.
pub fn run_mock_server() {
    let hang = std::env::var("MOCK_ACP_HANG").is_ok();
    let updates: usize = std::env::var("MOCK_ACP_UPDATES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    // When set, all chunks share one messageId (a multi-chunk agent
    // message) instead of getting a unique id each.
    let same_msg = std::env::var("MOCK_ACP_SAME_MSG").is_ok();
    // Write own PID to a file so the hard-kill test can check the process
    // is dead after cancel. Best-effort.
    if let Ok(pid_file) = std::env::var("MOCK_ACP_PID_FILE") {
        let _ = std::fs::write(&pid_file, std::process::id().to_string());
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut session_id = "mock-session-1".to_string();
    let mut msg_id: u64 = 0;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let method = v["method"].as_str().unwrap_or("").to_string();
        let id = v["id"].clone();
        let params = v["params"].clone();
        match method.as_str() {
            "initialize" => {
                let pv = params["protocolVersion"].as_str().unwrap_or("2025-03-26");
                send(
                    &mut out,
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": { "protocolVersion": pv, "agentCapabilities": {} },
                    }),
                );
            }
            "session/new" => {
                session_id = format!("mock-session-{}", msg_id);
                send(
                    &mut out,
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": { "sessionId": session_id },
                    }),
                );
            }
            "session/prompt" => {
                if hang {
                    // Never answer — cancel must break the read loop via
                    // the token + session/cancel notification.
                    std::thread::sleep(std::time::Duration::from_secs(120));
                    break;
                }
                for i in 0..updates {
                    let notif = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": session_id,
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": format!("mock output chunk {i}") },
                                "messageId": if same_msg { "same-msg".to_string() } else { format!("m{i}") },
                            },
                        },
                    });
                    send(&mut out, notif);
                }
                send(
                    &mut out,
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": { "stopReason": "end_turn" },
                    }),
                );
            }
            "session/cancel" => {
                send(
                    &mut out,
                    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
                );
                break;
            }
            "authenticate" => {
                send(
                    &mut out,
                    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }),
                );
            }
            _ => {
                if v["id"].is_number() {
                    send(
                        &mut out,
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32601, "message": "method not found" },
                        }),
                    );
                }
                // Notification without an id: ignore.
            }
        }
        msg_id += 1;
    }
}

fn send(out: &mut std::io::Stdout, value: serde_json::Value) {
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}
