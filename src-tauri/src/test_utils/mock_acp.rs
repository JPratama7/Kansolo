//! Mock ACP agent for testing the runner end-to-end.
//!
//! Self-spawn pattern: the runner tests point `AcpAgentConfig` at the lib
//! test harness binary (`std::env::current_exe()`) with
//! `--exact mock_acp_server` and `MOCK_ACP_BINARY=1`. The `mock_acp_server`
//! test in `runner.rs` then calls [`run_mock_server`] instead of asserting.
//! The mock speaks just enough Agent Client Protocol v1 over newline-
//! delimited JSON-RPC on stdio: initialize, session/new, session/prompt
//! (streaming `session/update` text chunks, then `end_turn`), and
//! session/cancel. Env knobs: `MOCK_ACP_UPDATES` (chunks per prompt,
//! default 1), `MOCK_ACP_HANG` (never answer session/prompt — for the
//! cancel test).

use std::io::{BufRead, Write};

/// Run the mock agent server loop on stdin/stdout until EOF or cancel.
pub fn run_mock_server() {
    let hang = std::env::var("MOCK_ACP_HANG").is_ok();
    let updates: usize = std::env::var("MOCK_ACP_UPDATES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
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
                let pv = params["protocolVersion"]
                    .as_str()
                    .unwrap_or("2025-03-26");
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
                    // Never answer — the cancel path must break the read
                    // loop via the token + session/cancel notification.
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
                                "messageId": format!("m{i}"),
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
                send(&mut out, serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                break;
            }
            "authenticate" => {
                send(&mut out, serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }));
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
