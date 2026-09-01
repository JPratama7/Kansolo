//! CLI module: hand-rolled CLI for the tasker ACP runtime.
//!
//! Subcommands: run, status, cancel, list, merge, cleanup, agents, skills.
//! No clap — args are parsed by hand from `std::env::args`.
//!
//! Entry point: `cli::dispatch_and_exit()`, called from `main.rs` when the
//! first CLI arg matches a known subcommand.

use std::path::PathBuf;
use std::str::FromStr;

use crate::db::{self, agent_runs, agents, cards, now_iso, open_db_path};
use crate::error::AcpError;
use crate::runner::{self, RunUpdate};
use crate::skills;
use crate::worktree::WorktreeManager;

/// Known CLI subcommands. Used by `main.rs` to decide CLI vs GUI mode.
pub const SUBCOMMANDS: &[&str] = &[
    "run", "status", "cancel", "list", "merge", "cleanup", "agents", "skills", "-h", "--help",
    "help",
];

/// Entry point: parse args, dispatch, exit with code.
/// Called from `main.rs` when the first arg is a known CLI subcommand.
#[tokio::main]
pub async fn dispatch_and_exit(args: &[String]) -> ! {
    if args.is_empty() {
        usage();
        std::process::exit(2);
    }
    let result = dispatch(args).await;
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {}", e.message);
            std::process::exit(1);
        }
    }
}

pub fn usage() {
    eprintln!(
        "tasker <command> [args]\n\
         commands:\n  \
           run <card_id> [--agent <name>] [--skill <name>]... [--input <text>]\n  \
           status [card_id]\n  \
           cancel <card_id>\n  \
           list\n  \
           merge <card_id> [--yes] [--prune]\n  \
           cleanup [--prune]\n  \
           agents [add|edit|remove ...]\n  \
           skills"
    );
}

/// Resolve the tasker DB path: `TASKER_CONFIG_DIR` env var >
/// `${XDG_CONFIG_HOME:-$HOME/.config}/tasker/tasker.db`.
fn resolve_db_path() -> Result<PathBuf, AcpError> {
    if let Ok(dir) = std::env::var("TASKER_CONFIG_DIR") {
        return Ok(PathBuf::from(dir).join("tasker.db"));
    }
    config_dir()
        .map(|d| d.join("tasker").join("tasker.db"))
        .ok_or_else(|| AcpError::internal("cannot resolve config dir; set TASKER_CONFIG_DIR"))
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(".config"))
        })
}

/// Open the DB, run migrations, and seed the built-in claude-code agent.
fn open_and_init() -> Result<rusqlite::Connection, AcpError> {
    let path = resolve_db_path()?;
    let conn = open_db_path(&path)?;
    db::run_migrations(&conn)?;
    agents::upsert_agent(
        &conn,
        "claude-code",
        "",
        "Claude Code via ACP SDK",
        true,
        true,
        &[],
    )?;
    Ok(conn)
}

async fn dispatch(args: &[String]) -> Result<i32, AcpError> {
    match args[0].as_str() {
        "run" => cmd_run(&args[1..]).await,
        "status" => cmd_status(&args[1..]).await,
        "cancel" => cmd_cancel(&args[1..]).await,
        "list" => cmd_list(&args[1..]).await,
        "merge" => cmd_merge(&args[1..]).await,
        "cleanup" => cmd_cleanup(&args[1..]).await,
        "agents" => cmd_agents(&args[1..]).await,
        "skills" => cmd_skills(&args[1..]).await,
        "-h" | "--help" | "help" => {
            usage();
            Ok(0)
        }
        other => {
            eprintln!("error: unknown command '{other}'");
            usage();
            Ok(2)
        }
    }
}

/// Parsed `--flag value` and `--flag=value` options. Returns a map of
/// flag → last value, plus the list of positional args (in order).
struct ParsedArgs {
    positional: Vec<String>,
    flags: std::collections::HashMap<String, Vec<String>>,
    bool_flags: std::collections::HashSet<String>,
}

/// Parse with `lexopt`: `--flag value`, `--flag=value`, `--bool`, and bare
/// positionals. Repeated flags collect into a Vec.
fn parse_args(args: &[String], bool_flags: &[&str]) -> Result<ParsedArgs, AcpError> {
    use lexopt::prelude::*;
    let mut positional = Vec::new();
    let mut flags: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut bool_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let bool_lookup: std::collections::HashSet<&str> = bool_flags.iter().copied().collect();

    let mut parser = lexopt::Parser::from_args(args.iter().cloned());
    while let Some(arg) = parser
        .next()
        .map_err(|e| AcpError::validation(e.to_string()))?
    {
        match arg {
            Long(raw_name) => {
                let name = raw_name.to_string();
                drop(arg);
                if bool_lookup.contains(name.as_str()) {
                    bool_set.insert(name);
                } else if let Ok(value) = parser.value() {
                    flags
                        .entry(name)
                        .or_default()
                        .push(value.to_string_lossy().into_owned());
                } else {
                    bool_set.insert(name);
                }
            }
            Short(_) => {
                return Err(AcpError::validation("short flags are not supported"));
            }
            Value(value) => positional.push(value.to_string_lossy().into_owned()),
        }
    }
    Ok(ParsedArgs {
        positional,
        flags,
        bool_flags: bool_set,
    })
}

fn last_flag<'a>(
    flags: &'a std::collections::HashMap<String, Vec<String>>,
    key: &str,
) -> Option<&'a str> {
    flags.get(key).and_then(|v| v.last()).map(|s| s.as_str())
}

/// `run <card_id> [--agent <name>] [--skill <name>]... [--input <text>]`
async fn cmd_run(args: &[String]) -> Result<i32, AcpError> {
    let parsed = parse_args(args, &[])?;
    let card_id = parsed
        .positional
        .first()
        .ok_or_else(|| {
            AcpError::validation("usage: run <card_id> [--agent ...] [--skill ...] [--input ...]")
        })?
        .clone();

    let (run_id, repo_path, prompt, skill_names, acp_agent) = {
        let conn = open_and_init()?;
        // Resolve agent: --agent flag > acp_default_agent setting > error.
        let agent_name = match last_flag(&parsed.flags, "agent") {
            Some(n) => n.to_string(),
            None => conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'acp_default_agent'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .map_err(|_| {
                    AcpError::validation(
                        "no agent specified; pass --agent <name> or set acp_default_agent",
                    )
                })?,
        };

        let agent = agents::get_agent(&conn, &agent_name)?
            .ok_or_else(|| AcpError::not_found(format!("Agent '{agent_name}' not found")))?;
        if !agent.enabled {
            return Err(AcpError::validation(format!(
                "Agent '{agent_name}' is disabled"
            )));
        }

        if agent_runs::is_card_locked(&conn, &card_id) {
            return Err(AcpError::locked(format!(
                "Card '{card_id}' already has an active agent run."
            )));
        }

        let card = cards::get_card_by_id(&conn, &card_id)?
            .ok_or_else(|| AcpError::not_found(format!("Card '{card_id}' not found")))?;
        let repo_path = cards::resolve_repo_path(&conn, &card)?;

        let acp_agent = if agent.built_in && agent_name == "claude-code" {
            agent_client_protocol::AcpAgent::claude_agent()
        } else {
            agent_client_protocol::AcpAgent::from_str(&agent.command)
                .map_err(|e| AcpError::internal(format!("Invalid agent command: {e}")))?
        };

        // Resolve skills: --skill flags (filtered against agent's skills) > all agent skills.
        let sn: Vec<String> = match parsed.flags.get("skill") {
            Some(names) => agent
                .skills
                .iter()
                .filter(|s| names.iter().any(|n| n == *s))
                .cloned()
                .collect(),
            None => agent.skills.clone(),
        };
        let loaded = skills::load_skills(&sn);
        let skills_section = runner::build_skills_section(&loaded);

        // Build prompt: skills section + card title/description + optional --input.
        let mut card_body = if card.description.is_empty() {
            card.title.clone()
        } else {
            format!("{}\n\n{}", card.title, card.description)
        };
        if let Some(extra) = last_flag(&parsed.flags, "input") {
            card_body.push_str("\n\n---\n\n");
            card_body.push_str(extra);
        }
        let prompt = if skills_section.is_empty() {
            card_body
        } else {
            format!("{}\n\n---\n\n{}", skills_section, card_body)
        };

        let run_id = uuid::Uuid::new_v4().to_string();
        agent_runs::insert_run(
            &conn,
            &run_id,
            &card_id,
            &agent_name,
            "/tmp/pending",
            "agent/pending",
            "pending",
            &sn,
        )?;
        (run_id, repo_path, prompt, sn, acp_agent)
    };

    // Create worktree + spawn SDK session (async). Stream updates to stdout.
    let exit_code = spawn_and_stream(
        &run_id,
        &card_id,
        &repo_path,
        &prompt,
        skill_names,
        acp_agent,
    )
    .await?;
    Ok(exit_code)
}

/// Create the worktree, spawn the ACP SDK session, stream updates to stdout,
/// and update the DB with the terminal status. Returns exit code (0 completed,
/// 1 failed/cancelled).
async fn spawn_and_stream(
    run_id: &str,
    card_id: &str,
    repo_path: &str,
    prompt: &str,
    _skill_names: Vec<String>,
    acp_agent: agent_client_protocol::AcpAgent,
) -> Result<i32, AcpError> {
    let wt_mgr = WorktreeManager::new(&PathBuf::from(repo_path));
    let worktree = wt_mgr.create(card_id).await?;

    // Persist the worktree path/branch/repo_root on the run row, replacing
    // the placeholder values written by `insert_run`.
    {
        let conn = open_and_init()?;
        agent_runs::set_worktree_info(
            &conn,
            run_id,
            &worktree.path.to_string_lossy(),
            &worktree.branch,
            repo_path,
        )?;
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<RunUpdate>();
    let cwd = worktree.path.clone();
    let prompt = prompt.to_string();

    let join = tokio::spawn(async move {
        let tx_for_err = tx.clone();
        let result = agent_client_protocol::Client
            .connect_with(acp_agent, move |cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>| {
                let tx = tx.clone();
                let cwd = cwd.clone();
                let prompt = prompt.clone();
                async move {
                    let mut session = cx.build_session(&cwd).block_task().start_session().await?;
                    let _ = tx.send(RunUpdate::SessionId {
                        session_id: session.session_id().0.to_string(),
                    });
                    session.send_prompt(&prompt)?;
                    loop {
                        match session.read_update().await {
                            Ok(msg) => {
                                use agent_client_protocol::SessionMessage;
                                match msg {
                                    SessionMessage::SessionMessage(dispatch) => {
                                        // CLI has no interactive UI, so auto-respond
                                        // to permission requests with the allow option.
                                        // Without this the agent hangs forever waiting
                                        // for a response that never comes.
                                        let method = dispatch.method().to_string();
                                        if method == "session/request_permission" {
                                            use agent_client_protocol::schema::v1::RequestPermissionRequest;
                                            match dispatch.into_request::<RequestPermissionRequest>() {
                                                Ok(Ok((req, responder))) => {
                                                    let option_id = runner::pick_allow_option(&req.options);
                                                    let description = runner::summarize_tool_call(&req.tool_call);
                                                    let _ = tx.send(RunUpdate::PermissionRequest {
                                                        request_id: req.tool_call.tool_call_id.0.to_string(),
                                                        description,
                                                    });
                                                    use agent_client_protocol::schema::v1::{
                                                        RequestPermissionResponse,
                                                        RequestPermissionOutcome,
                                                        SelectedPermissionOutcome,
                                                    };
                                                    let _ = responder.respond(
                                                        RequestPermissionResponse::new(
                                                            RequestPermissionOutcome::Selected(
                                                                SelectedPermissionOutcome::new(option_id),
                                                            ),
                                                        ),
                                                    );
                                                }
                                                Ok(Err(_)) | Err(_) => {
                                                    let _ = tx.send(RunUpdate::SessionUpdate {
                                                        text: "permission request: failed to parse".to_string(),
                                                    });
                                                }
                                            }
                                        } else if method == "session/update" {
                                            use agent_client_protocol::schema::v1::SessionNotification;
                                            match dispatch.into_notification::<SessionNotification>() {
                                                Ok(Ok(notif)) => {
                                                    if let Some(text) = runner::format_session_update(&notif.update) {
                                                        let _ = tx.send(RunUpdate::SessionUpdate { text });
                                                    }
                                                }
                                                Ok(Err(_)) | Err(_) => {}
                                            }
                                        } else {
                                            let _ = tx.send(RunUpdate::SessionUpdate {
                                                text: format!("(unhandled: {method})"),
                                            });
                                        }
                                    }
                                    SessionMessage::StopReason(reason) => {
                                        let _ = tx.send(RunUpdate::Completed {
                                            output: String::new(),
                                            stop_reason: format!("{:?}", reason),
                                        });
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(RunUpdate::Failed {
                                    error: e.to_string(),
                                });
                                break;
                            }
                        }
                    }
                    Ok(())
                }
            })
            .await;
        if let Err(e) = result {
            let _ = tx_for_err.send(RunUpdate::Failed {
                error: e.to_string(),
            });
        }
    });

    // Drain updates to stdout + track terminal state.
    let mut exit_code = 1;
    while let Some(update) = rx.recv().await {
        match update {
            RunUpdate::SessionId { session_id } => {
                println!("session: {session_id}");
            }
            RunUpdate::SessionUpdate { text } => {
                println!("{text}");
            }
            RunUpdate::Completed { stop_reason, .. } => {
                println!("\n[completed] stop_reason={stop_reason}");
                exit_code = 0;
                break;
            }
            RunUpdate::Failed { error } => {
                eprintln!("\n[failed] {error}");
                exit_code = 1;
                break;
            }
            RunUpdate::Cancelled => {
                eprintln!("\n[cancelled]");
                exit_code = 1;
                break;
            }
            RunUpdate::PermissionRequest { description, .. } => {
                eprintln!("[permission request] {description} (auto-approved in CLI)");
            }
            RunUpdate::PermissionTimeout => {
                eprintln!("[permission request timed out]");
            }
            RunUpdate::WaitingForInput { stop_reason } => {
                // CLI's session handler sends Completed directly (no
                // interactive mode), so this should never fire. Handle
                // gracefully just in case.
                eprintln!("\n[waiting for input] stop_reason={stop_reason}");
                exit_code = 0;
                break;
            }
        }
    }
    // Wait for the spawned task to finish.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), join).await;

    // Update DB with terminal status.
    let conn = open_and_init()?;
    let (status, error) = if exit_code == 0 {
        ("completed", None)
    } else {
        ("failed", Some("CLI: run did not complete successfully"))
    };
    agent_runs::update_status(&conn, run_id, status, None, None, error, Some(&now_iso()))?;
    Ok(exit_code)
}

/// `status [card_id]` — print a single run for a card, or list recent 20.
async fn cmd_status(args: &[String]) -> Result<i32, AcpError> {
    let conn = open_and_init()?;
    if let Some(card_id) = args.first() {
        let run = agent_runs::get_active_run(&conn, card_id)?.or_else(|| {
            // Fall back to the most recent run for the card (any status).
            agent_runs::get_latest_run_for_card(&conn, card_id)
                .ok()
                .flatten()
        });
        match run {
            Some(r) => {
                print_run(&r);
                Ok(0)
            }
            None => {
                println!("no runs for card {card_id}");
                Ok(0)
            }
        }
    } else {
        let runs = agent_runs::list_recent(&conn, 20)?;
        if runs.is_empty() {
            println!("no runs");
        } else {
            for r in runs {
                print_run(&r);
            }
        }
        Ok(0)
    }
}

fn print_run(r: &agent_runs::AgentRun) {
    println!(
        "{:<12} card={:<12} agent={:<12} status={:<10} created={}",
        r.id, r.card_id, r.agent_name, r.status, r.created_at
    );
    if let Some(e) = &r.error {
        println!("  error: {e}");
    }
    if let Some(m) = &r.merged_at {
        println!("  merged_at: {m}");
    }
}

/// `cancel <card_id>` — cancel active run (update DB to "cancelled").
async fn cmd_cancel(args: &[String]) -> Result<i32, AcpError> {
    let card_id = args
        .first()
        .ok_or_else(|| AcpError::validation("usage: cancel <card_id>"))?;
    let conn = open_and_init()?;
    let run = agent_runs::get_active_run(&conn, card_id)?
        .ok_or_else(|| AcpError::not_found(format!("no active run for card {card_id}")))?;
    agent_runs::update_status(
        &conn,
        &run.id,
        "cancelled",
        None,
        Some("user_cancelled"),
        None,
        Some(&now_iso()),
    )?;
    println!("cancelled run {}", run.id);
    Ok(0)
}

/// `list` — list cards with their lock/run state.
async fn cmd_list(_args: &[String]) -> Result<i32, AcpError> {
    let conn = open_and_init()?;
    let mut stmt = conn
        .prepare(
            r#"SELECT c.id, c.title, c."column", c.source, ts.label
           FROM cards c LEFT JOIN tree_sources ts ON ts.id = c.tree_source_id
           ORDER BY c."column", c.position ASC"#,
        )
        .map_err(|e| AcpError::internal(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| AcpError::internal(e.to_string()))?;
    // Collect first so the statement borrow ends before we query is_card_locked.
    let mut cards: Vec<(String, String, String, String, Option<String>)> = Vec::new();
    for r in rows {
        cards.push(r.map_err(|e| AcpError::internal(e.to_string()))?);
    }
    drop(stmt);
    println!(
        "{:<12} {:<24} {:<10} {:<8} {:<6} {}",
        "id", "title", "column", "source", "locked", "tree"
    );
    for (id, title, column, source, tree) in cards {
        let locked = agent_runs::is_card_locked(&conn, &id);
        println!(
            "{:<12} {:<24} {:<10} {:<8} {:<6} {}",
            id,
            truncate(&title, 24),
            column,
            source,
            if locked { "yes" } else { "no" },
            tree.unwrap_or_else(|| "-".to_string())
        );
    }
    Ok(0)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    // Slice on a char boundary: take n-1 chars, then append an ellipsis.
    // `&s[..n]` would panic on multi-byte UTF-8 if n landed mid-codepoint.
    let cut = s
        .char_indices()
        .take(n.saturating_sub(1))
        .last()
        .map(|(i, _)| &s[..i])
        .unwrap_or(s);
    format!("{cut}…")
}

/// `merge <card_id> [--yes] [--prune]` — print diff summary, prompt [y/N]
/// unless --yes, call merge_branch, set merged_at on success.
async fn cmd_merge(args: &[String]) -> Result<i32, AcpError> {
    let parsed = parse_args(args, &["yes", "prune"])?;
    let card_id = parsed
        .positional
        .first()
        .ok_or_else(|| AcpError::validation("usage: merge <card_id> [--yes] [--prune]"))?
        .clone();
    let yes = parsed.bool_flags.contains("yes");
    let prune = parsed.bool_flags.contains("prune");

    let (repo_path, run_id) = {
        let conn = open_and_init()?;
        // Find the run: active first, then most recent. The repo root comes
        // from the run row (persisted by set_worktree_info when the worktree
        // was created), not from the card.
        let run = agent_runs::get_active_run(&conn, &card_id)?
            .or(agent_runs::get_latest_run_for_card(&conn, &card_id)?)
            .ok_or_else(|| AcpError::not_found("No run found for card"))?;
        let repo_root = run.repo_root.as_ref().ok_or_else(|| {
            AcpError::validation(
                "Run has no repo_root (may be a pre-migration run without a repo path)",
            )
        })?;
        (repo_root.clone(), run.id)
    };

    // Print diff summary.
    let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo_path));
    let diff = wt_mgr.diff_main(&card_id).await?;
    let lines = diff.lines().count();
    let bytes = diff.len();
    println!("diff: {lines} lines, {bytes} bytes");
    if !diff.is_empty() {
        // Print first 40 lines as a summary.
        for line in diff.lines().take(40) {
            println!("  {line}");
        }
        if lines > 40 {
            println!("  ... ({} more lines)", lines - 40);
        }
    }

    if !yes {
        print!("merge agent/c{} into main? [y/N] ", card_id);
        use std::io::Write;
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("aborted");
            return Ok(0);
        }
    }

    let result = wt_mgr.merge_branch(&card_id).await?;
    if result.success {
        let conn = open_and_init()?;
        agent_runs::set_merged(&conn, &run_id, &now_iso())?;
        println!("merged");
        if prune {
            let _ = wt_mgr.remove(&card_id).await;
            println!("worktree pruned");
        }
        Ok(0)
    } else {
        eprintln!("merge failed (conflicts):");
        for c in &result.conflicts {
            eprintln!("  conflict: {c}");
        }
        Ok(1)
    }
}

/// `cleanup [--prune]` — call cleanup_dangling from runner module.
async fn cmd_cleanup(_args: &[String]) -> Result<i32, AcpError> {
    let conn = open_and_init()?;
    let reaped = runner::cleanup_dangling(&conn)?;
    if reaped.is_empty() {
        println!("no dangling runs");
    } else {
        for id in &reaped {
            println!("reaped run {id}");
        }
    }
    println!("reaped {} dangling run(s)", reaped.len());
    Ok(0)
}

/// `skills` — list available skills from disk.
async fn cmd_skills(_args: &[String]) -> Result<i32, AcpError> {
    let skills = skills::list_skills();
    if skills.is_empty() {
        println!("no skills found");
        return Ok(0);
    }
    println!("{:<20} {}", "name", "description");
    for s in skills {
        println!("{:<20} {}", s.name, s.description);
    }
    Ok(0)
}

/// `agents` — list registered agents, or `agents add|edit|remove`.
async fn cmd_agents(args: &[String]) -> Result<i32, AcpError> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "" | "list" => {
            let conn = open_and_init()?;
            let agents = agents::list_agents(&conn)?;
            if agents.is_empty() {
                println!("no agents");
                return Ok(0);
            }
            println!(
                "{:<16} {:<24} {:<8} {:<8} {}",
                "name", "command", "built_in", "enabled", "skills"
            );
            for a in agents {
                println!(
                    "{:<16} {:<24} {:<8} {:<8} {}",
                    a.name,
                    truncate(&a.command, 24),
                    if a.built_in { "yes" } else { "no" },
                    if a.enabled { "yes" } else { "no" },
                    a.skills.len()
                );
            }
            Ok(0)
        }
        "add" => cmd_agents_add(&args[1..]),
        "edit" => cmd_agents_edit(&args[1..]),
        "remove" => cmd_agents_remove(&args[1..]),
        other => {
            eprintln!("error: unknown agents subcommand '{other}'");
            eprintln!("usage: agents [list|add <name> --command <path> [--desc <text>] [--skill <name>]...|edit <name> ...|remove <name>]");
            Ok(2)
        }
    }
}

/// `agents add <name> --command <path> [--desc <text>] [--skill <name>]...`
fn cmd_agents_add(args: &[String]) -> Result<i32, AcpError> {
    let parsed = parse_args(args, &[])?;
    let name = parsed
        .positional
        .first()
        .ok_or_else(|| {
            AcpError::validation(
                "usage: agents add <name> --command <path> [--desc ...] [--skill ...]",
            )
        })?
        .clone();
    let command = last_flag(&parsed.flags, "command")
        .unwrap_or("")
        .to_string();
    let desc = last_flag(&parsed.flags, "desc").unwrap_or("").to_string();
    let skills: Vec<String> = parsed.flags.get("skill").cloned().unwrap_or_default();
    if name.is_empty() {
        return Err(AcpError::validation("Agent name cannot be empty"));
    }
    // Built-in agents (claude-code) have empty command — allowed.
    if command.is_empty() && name != "claude-code" {
        return Err(AcpError::validation(
            "Agent command cannot be empty for non-built-in agents",
        ));
    }
    let conn = open_and_init()?;
    let built_in = name == "claude-code";
    agents::insert_agent(&conn, &name, &command, &desc, built_in, true, &skills)?;
    println!("added agent {name}");
    Ok(0)
}

/// `agents edit <name> [--command <path>] [--desc <text>] [--skill <name>]...`
fn cmd_agents_edit(args: &[String]) -> Result<i32, AcpError> {
    let parsed = parse_args(args, &[])?;
    let name = parsed
        .positional
        .first()
        .ok_or_else(|| {
            AcpError::validation(
                "usage: agents edit <name> [--command ...] [--desc ...] [--skill ...]",
            )
        })?
        .clone();
    let conn = open_and_init()?;
    let existing = agents::get_agent(&conn, &name)?
        .ok_or_else(|| AcpError::not_found(format!("Agent '{name}' not found")))?;
    let command = last_flag(&parsed.flags, "command")
        .map(|s| s.to_string())
        .unwrap_or(existing.command);
    let desc = last_flag(&parsed.flags, "desc")
        .map(|s| s.to_string())
        .unwrap_or(existing.description);
    let skills: Vec<String> = parsed
        .flags
        .get("skill")
        .cloned()
        .unwrap_or(existing.skills);
    agents::update_agent(&conn, &name, &command, &desc, &skills)?;
    println!("updated agent {name}");
    Ok(0)
}

/// `agents remove <name>` — delete from DB (delete_runs=false by default).
fn cmd_agents_remove(args: &[String]) -> Result<i32, AcpError> {
    let parsed = parse_args(args, &[])?;
    let name = parsed
        .positional
        .first()
        .ok_or_else(|| AcpError::validation("usage: agents remove <name>"))?
        .clone();
    let conn = open_and_init()?;
    agents::delete_agent(&conn, &name, false)?;
    println!("removed agent {name}");
    Ok(0)
}
