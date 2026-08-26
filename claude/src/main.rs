use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use tally_common::agent_runtime::{
    backup_if_exists, env_enabled, evidence_summary, expand_home, first_mapping_by_key,
    first_string_by_key, first_value_by_key, home_dir, hook_command, light_git_state,
    parse_payload, random_hex, read_json_file, read_stdin, run_id, safe_slug, server_evidence,
    set_default, sha256_str, sha256_value, stable_id, utc_now, workspace_path, write_json_atomic,
    AuditSink, AuditSinkConfig, HeartbeatFiles,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const EVENTS: &[HookEvent] = &[
    HookEvent::new(
        "SessionStart",
        Some("*"),
        "Tally: recording Claude Code session start",
    ),
    HookEvent::new("UserPromptSubmit", None, "Tally: recording user prompt"),
    HookEvent::new("PreToolUse", Some("*"), "Tally: recording pre-tool action"),
    HookEvent::new(
        "PermissionRequest",
        Some("*"),
        "Tally: recording permission request",
    ),
    HookEvent::new(
        "PostToolUse",
        Some("*"),
        "Tally: recording post-tool result",
    ),
    HookEvent::new("PreCompact", Some("*"), "Tally: recording pre-compact"),
    HookEvent::new("PostCompact", Some("*"), "Tally: recording post-compact"),
    HookEvent::new(
        "SubagentStart",
        Some("*"),
        "Tally: recording subagent start",
    ),
    HookEvent::new("SubagentStop", Some("*"), "Tally: recording subagent stop"),
    HookEvent::new("Stop", None, "Tally: recording Claude Code turn end"),
    HookEvent::new(
        "SessionEnd",
        None,
        "Tally: recording Claude Code session end",
    ),
];

pub fn dispatch(arguments: Vec<String>) -> Result<i32> {
    let mut args = arguments.into_iter();
    match args.next().as_deref() {
        Some("hook") => {
            let event = args
                .next()
                .or_else(|| env::var("CLAUDE_HOOK_EVENT").ok())
                .unwrap_or_else(|| "unknown".to_string());
            record_hook_event(&event)?;
            Ok(0)
        }
        Some("heartbeat-daemon" | "daemon") => {
            run_heartbeat_daemon()?;
            Ok(0)
        }
        Some("forward-pending") => {
            tally_common::forward_pending(&onboarding_state_dir())?;
            Ok(0)
        }
        Some("install-desktop-hooks" | "install") => {
            let options =
                tally_common::parse_install_options(args.collect::<Vec<_>>(), "Claude Code")?;
            install_desktop_hooks(options)?;
            Ok(0)
        }
        Some("uninstall-desktop-hooks" | "uninstall") => {
            let config_path = tally_common::parse_config_path_options(args.collect::<Vec<_>>())?;
            uninstall_desktop_hooks(config_path)?;
            Ok(0)
        }
        Some("wrap") => wrap_claude(args.collect()),
        Some("--help" | "-h" | "help") => {
            print_help();
            Ok(0)
        }
        Some("--version" | "version") => {
            println!("tally-claude {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Some(event_name) => {
            record_hook_event(event_name)?;
            Ok(0)
        }
        None => Ok(0),
    }
}

fn print_help() {
    println!(
        "tally-claude {}\n\nCommands:\n  gui           Open the graphical installer\n  install --api-key <KEY> [--api-url <URL>] [--config-path <PATH>]\n                Install or update Claude Code hooks\n  uninstall [--config-path <PATH>]\n                Remove Tally hooks and local credentials\n  wrap [ARGS]   Run Claude Code through Tally\n  hook EVENT    Record a hook event\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn record_hook_event(event_type: &str) -> Result<()> {
    set_runtime_defaults();

    let raw = read_stdin()?;
    let payload = parse_payload(&raw);
    if env::var("TALLY_RUN_ID").unwrap_or_default().is_empty() {
        if let Some(run_id) = derive_run_id(&payload) {
            env::set_var("TALLY_RUN_ID", run_id);
        }
    }

    let sink = audit_sink("claude-hooks")?;
    let raw_ref = sink.private_payload(&payload)?;
    let observed_at = utc_now();
    let metadata = json!({
        "observed_at": observed_at,
        "hook_event": event_type,
        "cwd": env::current_dir()?.display().to_string(),
        "argv": env::args().collect::<Vec<_>>(),
        "raw_stdin_hash": sha256_str(&raw),
        "environment": scrub_environment(),
        "git_state": light_git_state(&workspace_path()),
    });
    let event_id = format!("evt_{}", random_hex(8));
    let event = json!({
        "schema_version": "tally-claude.v1",
        "event_id": event_id,
        "run_id": sink.run_id,
        "source": "claude-hooks",
        "event_type": event_type,
        "observed_at": observed_at,
        "payload_hash": raw_ref["hash"],
        "payload_uri": raw_ref["uri"],
        "metadata": metadata,
    });

    sink.append_jsonl("claude-hooks", &event)?;
    update_heartbeat_state(
        &sink,
        event_type,
        &payload,
        event["observed_at"].as_str().unwrap_or(&utc_now()),
    )?;

    let mut record = build_tally_record(&sink, event_type, &payload, &raw_ref, &metadata);
    record["record_id"] = Value::String(format!(
        "rec_{}",
        event["event_id"]
            .as_str()
            .unwrap_or("evt_unknown")
            .trim_start_matches("evt_")
    ));
    record["audit_event_id"] = event["event_id"].clone();
    sink.write_tally_record(&record)?;
    Ok(())
}

fn wrap_claude(args: Vec<String>) -> Result<i32> {
    set_runtime_defaults();

    let is_print_mode = matches!(args.first().map(String::as_str), Some("-p" | "--print"));
    if is_print_mode && env_enabled("TALLY_TEE_CLAUDE_STDIO", false) {
        run_claude_with_tee(&args)
    } else {
        let status = Command::new("claude").args(&args).status()?;
        Ok(status.code().unwrap_or(1))
    }
}

fn run_claude_with_tee(args: &[String]) -> Result<i32> {
    let stdio_dir = log_root().join("claude-stdio");
    fs::create_dir_all(&stdio_dir)?;
    let run_id = run_id();
    let stdout_log = stdio_dir.join(format!("{run_id}.stdout.log"));
    let stderr_log = stdio_dir.join(format!("{run_id}.stderr.log"));

    let mut child = Command::new("claude")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or("failed to capture claude stdout")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("failed to capture claude stderr")?;
    let stdout_thread = thread::spawn(move || tee_stream(stdout, stdout_log, false));
    let stderr_thread = thread::spawn(move || tee_stream(stderr, stderr_log, true));

    let status = child.wait()?;
    stdout_thread
        .join()
        .map_err(|_| "stdout tee thread panicked")??;
    stderr_thread
        .join()
        .map_err(|_| "stderr tee thread panicked")??;
    Ok(status.code().unwrap_or(1))
}

fn tee_stream<R: Read>(mut reader: R, log_path: PathBuf, stderr: bool) -> io::Result<()> {
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let mut buf = [0_u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        log.write_all(&buf[..n])?;
        if stderr {
            io::stderr().write_all(&buf[..n])?;
            io::stderr().flush()?;
        } else {
            io::stdout().write_all(&buf[..n])?;
            io::stdout().flush()?;
        }
    }
    Ok(())
}

fn update_heartbeat_state(
    sink: &AuditSink,
    event_type: &str,
    payload: &Value,
    observed_at: &str,
) -> Result<()> {
    tally_common::agent_runtime::update_heartbeat_state(
        sink,
        &HeartbeatFiles::new(&log_root(), &sink.run_id),
        "claude",
        event_type,
        extract_session_id(payload),
        observed_at,
    )
}

fn run_heartbeat_daemon() -> Result<()> {
    set_runtime_defaults();
    let sink = audit_sink("hook-heartbeat")?;
    tally_common::agent_runtime::run_heartbeat_daemon(
        &sink,
        &HeartbeatFiles::new(&log_root(), &sink.run_id),
    )
}

pub fn install_desktop_hooks(
    options: tally_common::InstallOptions,
) -> Result<tally_common::InstallReport> {
    set_runtime_defaults();

    let settings_path = effective_settings_path(options.config_path.as_deref());
    let state_dir = state_dir_for_settings_path(&settings_path);
    let installed_binary_path = installed_binary_path_for_settings_path(&settings_path);
    fs::create_dir_all(settings_path.parent().unwrap_or_else(|| Path::new(".")))?;
    tally_common::mark_tally_data_directory(&log_root())?;
    let source_binary = tally_common::installation_source_executable()?;
    let hook_bin = installed_binary_path.display().to_string();

    let mut config = if settings_path.exists() {
        read_json_file(&settings_path)?
    } else {
        json!({"hooks": {}})
    };
    if !config.is_object() {
        return Err(format!(
            "refusing to modify non-object JSON at {}",
            settings_path.display()
        )
        .into());
    }
    if !config.get("hooks").map(Value::is_object).unwrap_or(false) {
        config["hooks"] = json!({});
    }

    let backup = backup_if_exists(&settings_path)?;
    let settings_snapshot = tally_common::FileSnapshot::capture(&settings_path)?;
    let key_snapshot =
        tally_common::FileSnapshot::capture(&tally_common::api_key_path(&state_dir))?;
    let api_config_snapshot =
        tally_common::FileSnapshot::capture(&tally_common::config_path(&state_dir))?;
    let binary_snapshot = tally_common::FileSnapshot::capture(&installed_binary_path)?;
    remove_tally_hooks(&mut config);
    let hooks = config["hooks"]
        .as_object_mut()
        .expect("hooks object exists");
    for event in EVENTS {
        hooks
            .entry(event.name.to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("refusing to modify hooks.{}: not a list", event.name))?
            .push(event.to_hook_group(&hook_bin, &state_dir));
    }

    let install_result = (|| -> Result<()> {
        tally_common::install_executable(&source_binary, &installed_binary_path)?;
        tally_common::write_credentials(&state_dir, &options)?;
        write_json_atomic(&settings_path, &config)?;
        if let Err(error) =
            tally_common::remove_legacy_installed_executable(&settings_path, "tally-claude")
        {
            eprintln!("Warning: could not remove the previous hook executable: {error}");
        }
        Ok(())
    })();
    if let Err(error) = install_result {
        return Err(tally_common::install_error_with_rollback(
            error,
            &[
                &settings_snapshot,
                &key_snapshot,
                &api_config_snapshot,
                &binary_snapshot,
            ],
        ));
    }
    println!(
        "Installed Tally Claude Code hooks into {}",
        settings_path.display()
    );
    if let Some(backup) = backup.as_ref() {
        println!("Backed up previous settings file to {}", backup.display());
    }
    println!("Hook binary: {hook_bin}");
    println!("Logs: {}", log_root().display());
    println!(
        "Agent API key: stored securely at {}",
        tally_common::api_key_path(&state_dir).display()
    );
    println!("Ingest API: {}", options.api_url);
    let handshake_error = match tally_common::notify_client_connected(
        &options.api_key,
        &options.api_url,
        "claude-code",
    ) {
        Ok(()) => {
            println!("OpenOrigins dashboard connection confirmed.");
            None
        }
        Err(error) => {
            eprintln!("Warning: {}", tally_common::handshake_warning(&error));
            Some(error)
        }
    };
    Ok(tally_common::InstallReport {
        config_path: settings_path,
        state_dir,
        logs_path: log_root(),
        installed_binary_path,
        backup_path: backup,
        handshake_error,
        approval_required: false,
        approval_instructions: None,
        client_version: None,
    })
}

pub fn uninstall_desktop_hooks(config_path: Option<PathBuf>) -> Result<()> {
    uninstall_desktop_hooks_with_options(config_path, false).map(|_| ())
}

pub fn uninstall_desktop_hooks_with_options(
    config_path: Option<PathBuf>,
    remove_data: bool,
) -> Result<tally_common::UninstallReport> {
    set_runtime_defaults();
    let settings_path = effective_settings_path(config_path.as_deref());
    let state_dir = state_dir_for_settings_path(&settings_path);
    let logs_path = log_root();
    if settings_path.exists() {
        let mut config = read_json_file(&settings_path)?;
        if !config.is_object() || !config.get("hooks").map(Value::is_object).unwrap_or(false) {
            println!(
                "No Tally hooks found in {} (no hooks key present)",
                settings_path.display()
            );
        } else {
            let backup = backup_if_exists(&settings_path)?;
            let removed = remove_tally_hooks(&mut config);
            write_json_atomic(&settings_path, &config)?;
            println!(
                "Removed {removed} Tally hook handler(s) from {}",
                settings_path.display()
            );
            if let Some(backup) = backup {
                println!("Backed up previous settings file to {}", backup.display());
            }
        }
    } else {
        println!("No settings file found at {}", settings_path.display());
    }
    remove_local_credentials_for_settings_path(&settings_path)?;
    if remove_data {
        tally_common::remove_tally_data(&state_dir, &logs_path)?;
    }
    Ok(tally_common::UninstallReport {
        config_path: settings_path,
        queue_path: state_dir.join("forward-queue"),
        state_dir,
        logs_path,
        data_removed: remove_data,
    })
}

fn build_tally_record(
    sink: &AuditSink,
    event_type: &str,
    payload: &Value,
    raw_ref: &Value,
    metadata: &Value,
) -> Value {
    let session_id = extract_session_id(payload).unwrap_or_else(|| sink.run_id.clone());
    let prompt = first_string_by_key(
        payload,
        &["prompt", "user_prompt", "input", "text", "content"],
    );
    let raw_hash = raw_ref["hash"].clone();
    let raw_uri = raw_ref["uri"].clone();
    let observed_at = metadata["observed_at"].clone();

    match event_type {
        "SessionStart" => json!({
            "record_type": "SESSION_START",
            "schema_version": "0.2",
            "session_id": session_id,
            "agent_id": agent_id(),
            "agent_version": agent_version(),
            "principal": {"type": "human", "id": "[ARB] claude-user"},
            "authority_scope_hash": sha256_value(metadata),
            "authority_scope_uri": raw_uri,
            "authority_granted_at": observed_at,
            "session_started_at": metadata["observed_at"],
            "claude_hook_event": event_type,
            "raw_hook_hash": raw_hash,
        }),
        "UserPromptSubmit" => {
            let evidence =
                server_evidence(&prompt.map(Value::String).unwrap_or_else(|| payload.clone()));
            let summary = evidence_summary(&evidence, "User prompt submitted to Claude Code");
            json!({
                "record_type": "INSTRUCTION_RECEIVED",
                "schema_version": "0.2",
                "session_id": session_id,
                "instruction_id": stable_id("instr", payload),
                "sender": {"id": "[ARB] user", "signature": sha256_value(payload)},
                "instruction_hash": raw_hash,
                "instruction_uri": raw_uri,
                "instruction_received_at": observed_at,
                "context_snapshot_hash": sha256_value(&metadata["git_state"]),
                "context_snapshot_uri": raw_ref["uri"],
                "declared_intent": {
                    "summary": format!("[ARB] {summary}"),
                    "detail_hash": raw_ref["hash"],
                    "detail_uri": raw_ref["uri"],
                },
                "server_evidence": evidence,
                "claude_hook_event": event_type,
            })
        }
        "PreToolUse" | "PermissionRequest" => {
            let tool_params = first_mapping_by_key(
                payload,
                &["tool_input", "arguments", "args", "params", "input"],
            )
            .cloned()
            .unwrap_or_else(|| payload.clone());
            let evidence = server_evidence(&tool_params);
            json!({
                "record_type": "ACTION_TAKEN",
                "schema_version": "0.2",
                "session_id": session_id,
                "action_id": action_id(payload),
                "instruction_id": first_string_by_key(payload, &["instruction_id", "prompt_id", "turn_id", "turnId"])
                    .unwrap_or_else(|| stable_id("instr", &Value::String(session_id.clone()))),
                "action_type": if event_type == "PermissionRequest" { "decision" } else { "tool_call" },
                "tool": {
                    "server": first_string_by_key(payload, &["server", "server_name", "mcp_server"])
                        .unwrap_or_else(|| "claude-code".to_string()),
                    "name": first_string_by_key(payload, &["tool_name", "toolName", "name", "command"])
                        .unwrap_or_else(|| event_type.to_string()),
                    "params_hash": sha256_value(&tool_params),
                    "params_uri": raw_ref["uri"],
                },
                "pre_state_hash": sha256_value(&metadata["git_state"]),
                "pre_state_uri": raw_ref["uri"],
                "post_state_hash": Value::Null,
                "post_state_uri": Value::Null,
                "action_timestamp": observed_at,
                "deviance_flag": {"deviated": false, "delta_category": Value::Null, "delta_hash": Value::Null, "delta_uri": Value::Null},
                "server_evidence": evidence,
                "claude_hook_event": event_type,
                "raw_hook_hash": raw_ref["hash"],
            })
        }
        "PostToolUse" => {
            let has_error =
                first_string_by_key(payload, &["tool_error", "error", "exception"]).is_some();
            let evidence_source = first_value_by_key(
                payload,
                &[
                    "tool_response",
                    "tool_result",
                    "result",
                    "output",
                    "content",
                ],
            )
            .unwrap_or(payload);
            let evidence = server_evidence(evidence_source);
            let summary = evidence_summary(&evidence, "Claude Code reported a tool result");
            json!({
                "record_type": "RESULT_RECEIVED",
                "schema_version": "0.2",
                "session_id": session_id,
                "action_id": action_id(payload),
                "result_hash": raw_ref["hash"],
                "result_uri": raw_ref["uri"],
                "result_received_at": observed_at,
                "result_interpretation": {
                    "summary": format!("[ARB] {summary}"),
                    "detail_hash": raw_ref["hash"],
                    "detail_uri": raw_ref["uri"],
                },
                "exception": {
                    "occurred": has_error,
                    "type": first_string_by_key(payload, &["error_type", "type"]),
                    "description_hash": if has_error { raw_ref["hash"].clone() } else { Value::Null },
                    "description_uri": if has_error { raw_ref["uri"].clone() } else { Value::Null },
                },
                "server_evidence": evidence,
                "claude_hook_event": event_type,
            })
        }
        "Stop" => {
            let evidence_source = first_value_by_key(
                payload,
                &[
                    "last_assistant_message",
                    "response",
                    "result",
                    "output",
                    "content",
                ],
            )
            .unwrap_or(payload);
            json!({
                "record_type": "TURN_END",
                "schema_version": "0.2",
                "session_id": session_id,
                "turn_id": turn_id(payload),
                "outcome": "completed",
                "outcome_hash": raw_ref["hash"],
                "outcome_uri": raw_ref["uri"],
                "turn_ended_at": observed_at,
                "server_evidence": server_evidence(evidence_source),
                "claude_hook_event": event_type,
            })
        }
        "SessionEnd" => json!({
            "record_type": "SESSION_END",
            "schema_version": "0.2",
            "session_id": session_id,
            "outcome": "partial",
            "outcome_hash": raw_ref["hash"],
            "outcome_uri": raw_ref["uri"],
            "session_ended_at": observed_at,
            "session_end_reason": first_string_by_key(payload, &["reason"]),
            "claude_hook_event": event_type,
        }),
        _ => json!({
            "record_type": "CLAUDE_LIFECYCLE",
            "schema_version": "0.2",
            "session_id": session_id,
            "claude_hook_event": event_type,
            "event_hash": raw_ref["hash"],
            "event_uri": raw_ref["uri"],
            "observed_at": observed_at,
            "metadata": metadata,
        }),
    }
}

#[cfg(test)]
fn record_type_for_hook(event_type: &str) -> &'static str {
    match event_type {
        "SessionStart" => "SESSION_START",
        "UserPromptSubmit" => "INSTRUCTION_RECEIVED",
        "PreToolUse" | "PermissionRequest" => "ACTION_TAKEN",
        "PostToolUse" => "RESULT_RECEIVED",
        "Stop" => "TURN_END",
        "SessionEnd" => "SESSION_END",
        _ => "CLAUDE_LIFECYCLE",
    }
}

#[derive(Clone, Copy)]
struct HookEvent {
    name: &'static str,
    matcher: Option<&'static str>,
    status: &'static str,
}

impl HookEvent {
    const fn new(name: &'static str, matcher: Option<&'static str>, status: &'static str) -> Self {
        Self {
            name,
            matcher,
            status,
        }
    }

    fn to_hook_group(self, hook_bin: &str, state_dir: &Path) -> Value {
        let mut group = serde_json::Map::new();
        if let Some(matcher) = self.matcher {
            group.insert("matcher".to_string(), Value::String(matcher.to_string()));
        }
        group.insert(
            "hooks".to_string(),
            json!([{
                "type": "command",
                "command": hook_command(hook_bin, "claude", self.name, state_dir),
                "timeout": if self.name == "SessionEnd" { 3 } else { 15 },
                "statusMessage": self.status,
            }]),
        );
        Value::Object(group)
    }
}

fn audit_sink(source: &str) -> Result<AuditSink> {
    AuditSink::new(AuditSinkConfig {
        source,
        log_root: log_root(),
        run_id: run_id(),
        workspace: workspace_path(),
        forwarding_state_dir: onboarding_state_dir(),
        agent_id: agent_id(),
        heartbeat_client: "claude-code",
        event_schema: "tally-claude.v1",
    })
}

fn remove_tally_hooks(config: &mut Value) -> usize {
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut removed = 0;
    let mut empty_events = Vec::new();

    for (event, groups) in hooks.iter_mut() {
        let Some(groups_array) = groups.as_array_mut() else {
            continue;
        };
        groups_array.retain_mut(|group| {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|handler| {
                let command = handler.get("command").and_then(Value::as_str).unwrap_or("");
                let is_current_hook = (command.contains("tally-claude")
                    || command.contains(" claude hook "))
                    && command.contains(" hook ");
                !is_current_hook
            });
            removed += before - handlers.len();
            !handlers.is_empty()
        });
        if groups_array.is_empty() {
            empty_events.push(event.clone());
        }
    }
    for event in empty_events {
        hooks.remove(&event);
    }
    removed
}

fn set_runtime_defaults() {
    set_default("TALLY_LOG_ROOT", &default_log_root());
    set_default(
        "TALLY_WORKSPACE",
        &env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
            .to_string(),
    );
    set_default("TALLY_AGENT_ID", "claude-desktop");
    set_default("TALLY_AGENT_VERSION", "claude-code");
    set_default(
        "TALLY_HOOK_HEARTBEAT_SECONDS",
        &tally_common::DEFAULT_HEARTBEAT_INTERVAL_SECONDS.to_string(),
    );
}

fn extract_session_id(payload: &Value) -> Option<String> {
    first_string_by_key(
        payload,
        &[
            "session_id",
            "sessionId",
            "thread_id",
            "conversation_id",
            "conversationId",
        ],
    )
}

fn derive_run_id(payload: &Value) -> Option<String> {
    extract_session_id(payload)
        .or_else(|| env::var("CLAUDE_SESSION_ID").ok())
        .map(|value| safe_slug(&format!("claude_{value}"), "claude-session"))
}

fn action_id(payload: &Value) -> String {
    first_string_by_key(
        payload,
        &[
            "tool_use_id",
            "toolUseId",
            "action_id",
            "tool_call_id",
            "call_id",
            "id",
        ],
    )
    .map(|value| {
        if value.starts_with("act_") {
            value
        } else {
            format!("act_{value}")
        }
    })
    .unwrap_or_else(|| stable_id("act", payload))
}

fn turn_id(payload: &Value) -> String {
    first_string_by_key(payload, &["turn_id", "turnId"])
        .unwrap_or_else(|| stable_id("turn", payload))
}

fn scrub_environment() -> Value {
    let allowed: BTreeSet<&str> = [
        "CLAUDE_PROJECT_DIR",
        "HOME",
        "HOSTNAME",
        "LANG",
        "LC_ALL",
        "LOGNAME",
        "PATH",
        "PWD",
        "SHELL",
        "TERM",
        "USER",
    ]
    .into_iter()
    .collect();
    let denied = [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "COOKIE",
        "AUTH",
        "CREDENTIAL",
    ];
    let mut out = serde_json::Map::new();
    for (key, value) in env::vars() {
        let upper = key.to_uppercase();
        if denied.iter().any(|fragment| upper.contains(fragment)) {
            continue;
        }
        if allowed.contains(key.as_str()) || key.starts_with("TALLY_") || key.starts_with("CLAUDE_")
        {
            out.insert(key, Value::String(value.chars().take(500).collect()));
        }
    }
    Value::Object(out)
}

fn default_log_root() -> String {
    format!("{}/.tally-claude/logs", home_dir())
}

fn log_root() -> PathBuf {
    expand_home(&env::var("TALLY_LOG_ROOT").unwrap_or_else(|_| default_log_root()))
}

pub fn default_config_path() -> PathBuf {
    if let Ok(path) = env::var("TALLY_CLAUDE_SETTINGS_PATH") {
        return expand_home(&path);
    }
    expand_home(&format!("{}/.claude/settings.json", home_dir()))
}

fn effective_settings_path(config_path: Option<&Path>) -> PathBuf {
    config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_path)
}

fn onboarding_state_dir() -> PathBuf {
    if let Ok(path) = env::var("TALLY_STATE_DIR") {
        return expand_home(&path);
    }
    state_dir_for_settings_path(&default_config_path())
}

fn state_dir_for_settings_path(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tally")
        .join("logs")
        .join(".state")
}

pub fn default_state_dir() -> PathBuf {
    state_dir_for_settings_path(&default_config_path())
}

pub fn default_installed_binary_path() -> PathBuf {
    installed_binary_path_for_settings_path(&default_config_path())
}

fn installed_binary_path_for_settings_path(path: &Path) -> PathBuf {
    tally_common::installed_executable_path(path, "tally-claude")
}

fn remove_local_credentials_for_settings_path(path: &Path) -> Result<()> {
    let state_dir = state_dir_for_settings_path(path);
    for path in [
        tally_common::api_key_path(&state_dir),
        tally_common::config_path(&state_dir),
        installed_binary_path_for_settings_path(path),
        tally_common::legacy_installed_executable_path(path, "tally-claude"),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn agent_id() -> String {
    env::var("TALLY_AGENT_ID").unwrap_or_else(|_| "claude-desktop".to_string())
}

fn agent_version() -> String {
    env::var("TALLY_AGENT_VERSION").unwrap_or_else(|_| "claude-code".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_payload_keeps_json_and_wraps_raw_text() {
        assert_eq!(parse_payload(r#"{"session_id":"s1"}"#)["session_id"], "s1");
        assert_eq!(parse_payload("not json")["raw_stdin"], "not json");
        assert!(parse_payload("").as_object().unwrap().is_empty());
    }

    #[test]
    fn extracts_session_id_recursively() {
        let payload = json!({"outer": {"conversationId": "conv-123"}});
        assert_eq!(extract_session_id(&payload).as_deref(), Some("conv-123"));
    }

    #[test]
    fn maps_hook_events_to_record_types() {
        assert_eq!(record_type_for_hook("SessionStart"), "SESSION_START");
        assert_eq!(
            record_type_for_hook("UserPromptSubmit"),
            "INSTRUCTION_RECEIVED"
        );
        assert_eq!(record_type_for_hook("PreToolUse"), "ACTION_TAKEN");
        assert_eq!(record_type_for_hook("PostToolUse"), "RESULT_RECEIVED");
        assert_eq!(record_type_for_hook("Stop"), "TURN_END");
        assert_eq!(record_type_for_hook("SessionEnd"), "SESSION_END");
        assert_eq!(record_type_for_hook("Other"), "CLAUDE_LIFECYCLE");
    }

    #[test]
    fn installs_turn_and_session_end_hooks() {
        assert!(EVENTS.iter().any(|event| event.name == "Stop"));
        assert!(EVENTS.iter().any(|event| event.name == "SessionEnd"));
    }

    #[test]
    fn uses_tool_use_id_to_correlate_actions_and_results() {
        let pre = json!({"tool_use_id": "tool-123", "tool_input": {"command": "true"}});
        let post = json!({"tool_use_id": "tool-123", "tool_response": {"stdout": ""}});
        assert_eq!(action_id(&pre), "act_tool-123");
        assert_eq!(action_id(&pre), action_id(&post));
    }

    #[test]
    fn removes_current_tally_hooks_only() {
        let mut config = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [
                        {"type": "command", "command": "/bin/echo keep"},
                        {"type": "command", "command": "/tmp/tally-claude hook SessionStart"}
                    ]
                }]
            }
        });
        assert_eq!(remove_tally_hooks(&mut config), 1);
        let handlers = config["hooks"]["SessionStart"][0]["hooks"]
            .as_array()
            .unwrap();
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0]["command"], "/bin/echo keep");
    }

    #[test]
    fn safe_slug_has_fallback_and_limits_length() {
        assert_eq!(safe_slug("hello world!", "x"), "hello_world");
        assert_eq!(safe_slug("!!!", "fallback"), "fallback");
        assert!(safe_slug(&"a".repeat(200), "x").len() <= 96);
    }
}
