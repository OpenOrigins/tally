use fs2::FileExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use tally_common::agent_runtime::{
    backup_if_exists, expand_home, first_string_by_key, home_dir, hook_command, light_git_state,
    parse_payload, random_hex, read_json_file, read_stdin, safe_slug, set_default, sha256_str,
    stable_id, transcript_token_usage, utc_now, workspace_path, write_json_atomic, AuditSink,
    AuditSinkConfig, HeartbeatFiles,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Cursor's hooks.json is a flat map of event name -> array of hook entries
/// (`{"command", "type", "timeout"}`), unlike Claude Code's matcher/group
/// nesting or Codex's TOML tables. See https://cursor.com/docs/agent/hooks.
const EVENTS: &[HookEvent] = &[
    HookEvent::new("sessionStart", 15, "Tally: recording Cursor session start"),
    HookEvent::new("beforeSubmitPrompt", 15, "Tally: recording user prompt"),
    HookEvent::new("preToolUse", 15, "Tally: recording pre-tool action"),
    HookEvent::new("beforeShellExecution", 15, "Tally: recording shell command"),
    HookEvent::new("beforeMCPExecution", 15, "Tally: recording MCP tool call"),
    HookEvent::new("beforeReadFile", 15, "Tally: recording file read"),
    HookEvent::new("postToolUse", 15, "Tally: recording post-tool result"),
    HookEvent::new("postToolUseFailure", 15, "Tally: recording tool failure"),
    HookEvent::new("subagentStart", 15, "Tally: recording subagent start"),
    HookEvent::new("subagentStop", 15, "Tally: recording subagent stop"),
    HookEvent::new(
        "preCompact",
        15,
        "Tally: recording context window compaction",
    ),
    HookEvent::new("stop", 15, "Tally: recording Cursor turn end"),
    HookEvent::new("sessionEnd", 3, "Tally: recording Cursor session end"),
];

/// Per Cursor's hook contract, a hook must print a JSON object matching the
/// event's response schema and exit 0, or the action it gates may be blocked
/// (missing/invalid output on a permission-gated hook is treated as a deny).
/// Tally never wants to block the user's editor, so it always grants/continues.
fn response_for(event_type: &str) -> Value {
    match event_type {
        "preToolUse"
        | "beforeShellExecution"
        | "beforeMCPExecution"
        | "beforeReadFile"
        | "subagentStart" => json!({"permission": "allow"}),
        "beforeSubmitPrompt" => json!({"continue": true}),
        _ => json!({}),
    }
}

pub fn dispatch(arguments: Vec<String>) -> Result<i32> {
    let mut args = arguments.into_iter();
    match args.next().as_deref() {
        Some("hook") => {
            let event = args
                .next()
                .or_else(|| env::var("CURSOR_HOOK_EVENT").ok())
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
            let options = tally_common::parse_install_options(args.collect::<Vec<_>>(), "Cursor")?;
            install_desktop_hooks(options)?;
            Ok(0)
        }
        Some("uninstall-desktop-hooks" | "uninstall") => {
            let config_path = tally_common::parse_config_path_options(args.collect::<Vec<_>>())?;
            uninstall_desktop_hooks(config_path)?;
            Ok(0)
        }
        Some("--help" | "-h" | "help") => {
            print_help();
            Ok(0)
        }
        Some("--version" | "version") => {
            println!("tally-cursor {}", env!("CARGO_PKG_VERSION"));
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
        "tally-cursor {}\n\nCommands:\n  install --api-key <KEY> [--api-url <URL>] [--config-path <PATH>]\n                Install or update Cursor hooks\n  uninstall [--config-path <PATH>]\n                Remove Tally hooks and local credentials\n  hook EVENT    Record a hook event\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn record_hook_event(event_type: &str) -> Result<()> {
    let raw = read_stdin()?;
    let payload = parse_payload(&raw);
    set_runtime_defaults(&payload);
    if env::var("TALLY_RUN_ID").unwrap_or_default().is_empty() {
        if let Some(run_id) = derive_run_id(&payload) {
            env::set_var("TALLY_RUN_ID", run_id);
        }
    }

    let sink = audit_sink("cursor-hooks")?;
    let raw_ref = sink.private_payload(&payload)?;
    let observed_at = utc_now();
    let session_id = extract_session_id(&payload).unwrap_or_else(|| sink.run_id.clone());
    let token_usage = token_usage_for_event(&sink, &payload, &session_id);
    let metadata = json!({
        "observed_at": observed_at,
        "hook_event": event_type,
        "cwd": env::current_dir()?.display().to_string(),
        "argv": env::args().collect::<Vec<_>>(),
        "raw_stdin_hash": sha256_str(&raw),
        "environment": scrub_environment(),
        "git_state": light_git_state(&workspace_path()),
    });
    let event_id = format!("evt_{}", random_hex(16));
    let event = json!({
        "schema_version": "tally-cursor.v1",
        "event_id": event_id,
        "run_id": sink.run_id,
        "source": "cursor-hooks",
        "event_type": event_type,
        "observed_at": observed_at,
        "payload_hash": raw_ref["hash"],
        "payload_uri": raw_ref["uri"],
        "metadata": metadata,
    });

    sink.append_jsonl("cursor-hooks", &event)?;
    update_heartbeat_state(
        &sink,
        event_type,
        &payload,
        event["observed_at"].as_str().unwrap_or(&utc_now()),
    )?;

    let mut record = build_tally_record(&sink, event_type, &payload, &raw_ref, &metadata)?;
    record["record_id"] = Value::String(format!(
        "rec_{}",
        event["event_id"]
            .as_str()
            .unwrap_or("evt_unknown")
            .trim_start_matches("evt_")
    ));
    record["audit_event_id"] = event["event_id"].clone();
    record["token_usage"] = token_usage;
    sink.write_tally_record(&record)?;

    println!("{}", response_for(event_type));
    Ok(())
}

/// Cursor's `stop` hook (and others) do not report token usage, so this
/// first tries the fields Cursor is known to send on some payloads
/// directly, accumulating a running per-session total on disk (Cursor
/// reports usage per-turn, not cumulatively, unlike Claude Code's
/// transcript). Falling back to `transcript_path`, when present, mirrors
/// Claude Code's transcript-wide accounting and degrades gracefully to
/// `{"available": false}` if neither source has usable data.
fn token_usage_for_event(sink: &AuditSink, payload: &Value, session_id: &str) -> Value {
    let turn = turn_token_usage(payload);
    if turn["available"] == true {
        let session_cumulative = update_session_token_totals(&sink.state_dir, session_id, &turn)
            .unwrap_or_else(|_| json!({"available": false}));
        let mut result = turn;
        result["session_cumulative"] = session_cumulative;
        result["source"] = json!("hook_payload");
        return result;
    }
    if let Some(path) = first_string_by_key(payload, &["transcript_path"]) {
        let transcript_usage = transcript_token_usage(&expand_home(&path));
        if transcript_usage["available"] == true {
            let mut result = transcript_usage;
            result["source"] = json!("transcript");
            return result;
        }
    }
    json!({"available": false, "source": "unavailable"})
}

const TOKEN_FIELDS: [&str; 4] = [
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
];

fn turn_token_usage(payload: &Value) -> Value {
    let mut totals = serde_json::Map::new();
    let mut found = false;
    let mut total_tokens = 0_u64;
    for field in TOKEN_FIELDS {
        let value = payload.get(field).and_then(Value::as_u64).unwrap_or(0);
        if payload.get(field).is_some() {
            found = true;
        }
        total_tokens = total_tokens.saturating_add(value);
        totals.insert(field.to_string(), Value::from(value));
    }
    if !found {
        return json!({"available": false});
    }
    totals.insert("total_tokens".to_string(), Value::from(total_tokens));
    totals.insert("available".to_string(), Value::Bool(true));
    Value::Object(totals)
}

fn update_session_token_totals(state_dir: &Path, session_id: &str, turn: &Value) -> Result<Value> {
    if session_id.is_empty() {
        return Ok(json!({"available": false}));
    }
    fs::create_dir_all(state_dir)?;
    let path = state_dir.join("cursor-session-token-totals.json");
    let lock_path = state_dir.join("cursor-session-token-totals.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    let result = (|| -> Result<Value> {
        let mut all_totals = read_json_file(&path).unwrap_or_else(|_| json!({}));
        if !all_totals.is_object() {
            all_totals = json!({});
        }
        let mut session_totals = all_totals
            .get(session_id)
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        for field in TOKEN_FIELDS {
            let previous = session_totals
                .get(field)
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let turn_value = turn.get(field).and_then(Value::as_u64).unwrap_or(0);
            session_totals[field] = Value::from(previous.saturating_add(turn_value));
        }
        let total_tokens = TOKEN_FIELDS
            .iter()
            .map(|field| session_totals[*field].as_u64().unwrap_or(0))
            .sum::<u64>();
        session_totals["total_tokens"] = Value::from(total_tokens);
        session_totals["available"] = Value::Bool(true);
        all_totals[session_id] = session_totals.clone();
        write_json_atomic(&path, &all_totals)?;
        Ok(session_totals)
    })();
    FileExt::unlock(&lock)?;
    result
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
        "cursor",
        event_type,
        extract_session_id(payload),
        observed_at,
    )
}

fn run_heartbeat_daemon() -> Result<()> {
    set_runtime_defaults(&json!({}));
    let sink = audit_sink("hook-heartbeat")?;
    tally_common::agent_runtime::run_heartbeat_daemon(
        &sink,
        &HeartbeatFiles::new(&log_root(), &sink.run_id),
    )
}

pub fn install_desktop_hooks(
    options: tally_common::InstallOptions,
) -> Result<tally_common::InstallReport> {
    let config_path = effective_config_path(options.config_path.as_deref());
    let state_dir = state_dir_for_config_path(&config_path);
    let installed_binary_path = installed_binary_path_for_config_path(&config_path);
    fs::create_dir_all(config_path.parent().unwrap_or_else(|| Path::new(".")))?;
    tally_common::mark_tally_data_directory(&log_root())?;
    let source_binary = tally_common::installation_source_executable()?;
    let hook_bin = installed_binary_path.display().to_string();

    let mut config = if config_path.exists() {
        read_json_file(&config_path)?
    } else {
        json!({"version": 1, "hooks": {}})
    };
    if !config.is_object() {
        return Err(format!(
            "refusing to modify non-object JSON at {}",
            config_path.display()
        )
        .into());
    }
    if config.get("version").is_none() {
        config["version"] = Value::from(1);
    }
    if !config.get("hooks").map(Value::is_object).unwrap_or(false) {
        config["hooks"] = json!({});
    }

    let backup = backup_if_exists(&config_path)?;
    let config_snapshot = tally_common::FileSnapshot::capture(&config_path)?;
    let key_snapshot =
        tally_common::FileSnapshot::capture(&tally_common::api_key_path(&state_dir))?;
    let api_config_snapshot =
        tally_common::FileSnapshot::capture(&tally_common::config_path(&state_dir))?;
    let agent_id_snapshot = tally_common::FileSnapshot::capture(&state_dir.join("agent-id.txt"))?;
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
            .push(json!({
                "command": hook_command(&hook_bin, "cursor", event.name, &state_dir),
                "type": "command",
                "timeout": event.timeout,
            }));
    }

    let install_result = (|| -> Result<()> {
        tally_common::install_executable(&source_binary, &installed_binary_path)?;
        tally_common::load_or_create_agent_id(&state_dir, "cursor")?;
        tally_common::write_credentials(&state_dir, &options)?;
        write_json_atomic(&config_path, &config)?;
        Ok(())
    })();
    if let Err(error) = install_result {
        return Err(tally_common::install_error_with_rollback(
            error,
            &[
                &config_snapshot,
                &key_snapshot,
                &api_config_snapshot,
                &agent_id_snapshot,
                &binary_snapshot,
            ],
        ));
    }
    println!(
        "Installed Tally Cursor hooks into {}",
        config_path.display()
    );
    if let Some(backup) = backup.as_ref() {
        println!(
            "Backed up previous Cursor hooks file to {}",
            backup.display()
        );
    }
    println!("Hook binary: {hook_bin}");
    println!("Logs: {}", log_root().display());
    println!(
        "Agent API key: stored securely at {}",
        tally_common::api_key_path(&state_dir).display()
    );
    println!("Ingest API: {}", options.api_url);
    let handshake_error =
        match tally_common::notify_client_connected(&options.api_key, &options.api_url, "cursor") {
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
        config_path,
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
    let config_path = effective_config_path(config_path.as_deref());
    let state_dir = state_dir_for_config_path(&config_path);
    let logs_path = log_root();
    if config_path.exists() {
        let mut config = read_json_file(&config_path)?;
        if !config.is_object() || !config.get("hooks").map(Value::is_object).unwrap_or(false) {
            println!(
                "No Tally hooks found in {} (no hooks key present)",
                config_path.display()
            );
        } else {
            let backup = backup_if_exists(&config_path)?;
            let removed = remove_tally_hooks(&mut config);
            write_json_atomic(&config_path, &config)?;
            println!(
                "Removed {removed} Tally hook handler(s) from {}",
                config_path.display()
            );
            if let Some(backup) = backup {
                println!(
                    "Backed up previous Cursor hooks file to {}",
                    backup.display()
                );
            }
        }
    } else {
        println!("No Cursor hooks file found at {}", config_path.display());
    }
    remove_local_credentials_for_config_path(&config_path)?;
    if remove_data {
        tally_common::remove_tally_data(&state_dir, &logs_path)?;
    }
    Ok(tally_common::UninstallReport {
        config_path,
        journal_path: state_dir.join("journal"),
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
) -> Result<Value> {
    let session_id = extract_session_id(payload).unwrap_or_else(|| sink.run_id.clone());
    let installation_agent_id = agent_id()?;
    let version = agent_version();
    let action = action_id(payload);
    let turn = turn_id(payload);
    let profile = tally_common::records::HookRecordProfile {
        hook_field: "cursor_hook_event",
        lifecycle_record_type: "CURSOR_LIFECYCLE",
        default_tool_server: "cursor",
        prompt_summary_label: "User prompt submitted to Cursor",
        result_summary_label: "Cursor reported a tool result",
        tool_param_keys: &["tool_input", "arguments", "args", "params", "input"],
        instruction_id_keys: &["instruction_id", "prompt_id", "turn_id"],
        tool_server_keys: &["server", "server_name", "mcp_server_name"],
        tool_name_keys: &["tool_name", "toolName", "name"],
        error_keys: &["error", "tool_error", "exception", "failure_reason"],
    };
    let identity = tally_common::records::HookRecordIdentity {
        session_id: &session_id,
        agent_id: &installation_agent_id,
        agent_version: &version,
        action_id: &action,
        turn_id: &turn,
    };
    tally_common::records::build_hook_record(
        sink, &profile, &identity, event_type, payload, raw_ref, metadata,
    )
}

#[cfg(test)]
fn record_type_for_hook(event_type: &str) -> &'static str {
    match event_type {
        "sessionStart" => "SESSION_START",
        "beforeSubmitPrompt" => "INSTRUCTION_RECEIVED",
        "preToolUse" | "beforeShellExecution" | "beforeMCPExecution" | "beforeReadFile" => {
            "ACTION_TAKEN"
        }
        "postToolUse" | "postToolUseFailure" => "RESULT_RECEIVED",
        "stop" => "TURN_END",
        "sessionEnd" => "SESSION_END",
        "subagentStart" | "subagentStop" => "HANDOFF",
        _ => "CURSOR_LIFECYCLE",
    }
}

#[derive(Clone, Copy)]
struct HookEvent {
    name: &'static str,
    timeout: u32,
    #[allow(dead_code)]
    description: &'static str,
}

impl HookEvent {
    const fn new(name: &'static str, timeout: u32, description: &'static str) -> Self {
        Self {
            name,
            timeout,
            description,
        }
    }
}

fn audit_sink(source: &str) -> Result<AuditSink> {
    AuditSink::new(AuditSinkConfig {
        source,
        log_root: log_root(),
        run_id: run_id(),
        workspace: workspace_path(),
        forwarding_state_dir: onboarding_state_dir(),
        agent_id: agent_id()?,
        heartbeat_client: "cursor",
        event_schema: "tally-cursor.v1",
    })
}

fn remove_tally_hooks(config: &mut Value) -> usize {
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut removed = 0;
    let mut empty_events = Vec::new();
    for (event, entries) in hooks.iter_mut() {
        let Some(array) = entries.as_array_mut() else {
            continue;
        };
        let before = array.len();
        array.retain(|entry| {
            let command = entry.get("command").and_then(Value::as_str).unwrap_or("");
            !is_tally_hook_command(command)
        });
        removed += before - array.len();
        if array.is_empty() {
            empty_events.push(event.clone());
        }
    }
    for event in empty_events {
        hooks.remove(&event);
    }
    removed
}

fn is_tally_hook_command(command: &str) -> bool {
    (command.contains("tally-cursor") || command.contains(" cursor hook "))
        && command.contains(" hook ")
}

fn set_runtime_defaults(payload: &Value) {
    set_default("TALLY_LOG_ROOT", &default_log_root());
    let workspace_hint = payload
        .get("workspace_roots")
        .and_then(Value::as_array)
        .and_then(|roots| roots.first())
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .display()
                .to_string()
        });
    set_default("TALLY_WORKSPACE", &workspace_hint);
    set_default("TALLY_AGENT_ID", "cursor-desktop");
    set_default("TALLY_AGENT_VERSION", "cursor");
    set_default(
        "TALLY_HOOK_HEARTBEAT_SECONDS",
        &tally_common::DEFAULT_HEARTBEAT_INTERVAL_SECONDS.to_string(),
    );
}

fn extract_session_id(payload: &Value) -> Option<String> {
    first_string_by_key(
        payload,
        &[
            "conversation_id",
            "session_id",
            "sessionId",
            "thread_id",
            "parent_conversation_id",
        ],
    )
}

fn derive_run_id(payload: &Value) -> Option<String> {
    extract_session_id(payload).map(|value| safe_slug(&format!("cursor_{value}"), "cursor-session"))
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
    first_string_by_key(payload, &["turn_id", "turnId", "generation_id"])
        .unwrap_or_else(|| stable_id("turn", payload))
}

fn scrub_environment() -> Value {
    let allowed: BTreeSet<&str> = [
        "CURSOR_PROJECT_DIR",
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
        if allowed.contains(key.as_str()) || key.starts_with("TALLY_") || key.starts_with("CURSOR_")
        {
            out.insert(key, Value::String(value.chars().take(500).collect()));
        }
    }
    Value::Object(out)
}

fn default_log_root() -> String {
    format!("{}/.tally-cursor/logs", home_dir())
}

fn log_root() -> PathBuf {
    expand_home(&env::var("TALLY_LOG_ROOT").unwrap_or_else(|_| default_log_root()))
}

pub fn default_config_path() -> PathBuf {
    if let Ok(path) = env::var("CURSOR_HOOKS_PATH") {
        return expand_home(&path);
    }
    expand_home(&format!("{}/.cursor/hooks.json", home_dir()))
}

fn effective_config_path(config_path: Option<&Path>) -> PathBuf {
    config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_path)
}

fn onboarding_state_dir() -> PathBuf {
    if let Ok(path) = env::var("TALLY_STATE_DIR") {
        return expand_home(&path);
    }
    state_dir_for_config_path(&default_config_path())
}

fn state_dir_for_config_path(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tally")
        .join("logs")
        .join(".state")
}

pub fn default_state_dir() -> PathBuf {
    state_dir_for_config_path(&default_config_path())
}

pub fn default_installed_binary_path() -> PathBuf {
    installed_binary_path_for_config_path(&default_config_path())
}

pub fn installation_snapshot_paths(config_path: Option<&Path>) -> Vec<PathBuf> {
    let config_path = effective_config_path(config_path);
    let state_dir = state_dir_for_config_path(&config_path);
    vec![
        config_path.clone(),
        tally_common::api_key_path(&state_dir),
        tally_common::config_path(&state_dir),
        state_dir.join("agent-id.txt"),
        installed_binary_path_for_config_path(&config_path),
    ]
}

fn installed_binary_path_for_config_path(path: &Path) -> PathBuf {
    tally_common::installed_executable_path(path, "tally-cursor")
}

fn remove_local_credentials_for_config_path(path: &Path) -> Result<()> {
    let state_dir = state_dir_for_config_path(path);
    for path in [
        tally_common::api_key_path(&state_dir),
        tally_common::config_path(&state_dir),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    tally_common::remove_installed_executable(&installed_binary_path_for_config_path(path))
}

fn agent_id() -> Result<String> {
    tally_common::load_or_create_agent_id(&onboarding_state_dir(), "cursor")
}

fn agent_version() -> String {
    env::var("TALLY_AGENT_VERSION").unwrap_or_else(|_| "cursor".to_string())
}

fn run_id() -> String {
    tally_common::agent_runtime::run_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_payload_keeps_json_and_wraps_raw_text() {
        assert_eq!(
            parse_payload(r#"{"conversation_id":"c1"}"#)["conversation_id"],
            "c1"
        );
        assert_eq!(parse_payload("not json")["raw_stdin"], "not json");
        assert!(parse_payload("").as_object().unwrap().is_empty());
    }

    #[test]
    fn extracts_session_id_from_conversation_id() {
        let payload = json!({"conversation_id": "conv-123", "model": "gpt-test"});
        assert_eq!(extract_session_id(&payload).as_deref(), Some("conv-123"));
    }

    #[test]
    fn maps_hook_events_to_record_types() {
        assert_eq!(record_type_for_hook("sessionStart"), "SESSION_START");
        assert_eq!(
            record_type_for_hook("beforeSubmitPrompt"),
            "INSTRUCTION_RECEIVED"
        );
        assert_eq!(record_type_for_hook("preToolUse"), "ACTION_TAKEN");
        assert_eq!(record_type_for_hook("beforeShellExecution"), "ACTION_TAKEN");
        assert_eq!(record_type_for_hook("postToolUse"), "RESULT_RECEIVED");
        assert_eq!(record_type_for_hook("stop"), "TURN_END");
        assert_eq!(record_type_for_hook("sessionEnd"), "SESSION_END");
        assert_eq!(record_type_for_hook("subagentStart"), "HANDOFF");
        assert_eq!(record_type_for_hook("preCompact"), "CURSOR_LIFECYCLE");
    }

    #[test]
    fn responses_grant_permission_and_continuation_by_default() {
        assert_eq!(response_for("preToolUse"), json!({"permission": "allow"}));
        assert_eq!(
            response_for("beforeShellExecution"),
            json!({"permission": "allow"})
        );
        assert_eq!(
            response_for("beforeSubmitPrompt"),
            json!({"continue": true})
        );
        assert_eq!(response_for("stop"), json!({}));
    }

    #[test]
    fn uses_tool_use_id_to_correlate_actions_and_results() {
        let pre = json!({"tool_use_id": "tool-123", "tool_input": {"command": "true"}});
        let post = json!({"tool_use_id": "tool-123", "tool_output": {"stdout": ""}});
        assert_eq!(action_id(&pre), "act_tool-123");
        assert_eq!(action_id(&pre), action_id(&post));
    }

    #[test]
    fn removes_current_tally_hooks_only() {
        let mut config = json!({
            "version": 1,
            "hooks": {
                "sessionStart": [
                    {"command": "./keep.sh", "type": "command"},
                    {"command": "/tmp/tally-cursor cursor hook sessionStart", "type": "command"}
                ]
            }
        });
        assert_eq!(remove_tally_hooks(&mut config), 1);
        let handlers = config["hooks"]["sessionStart"].as_array().unwrap();
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0]["command"], "./keep.sh");
    }

    #[test]
    fn turn_token_usage_is_available_only_when_fields_are_present() {
        assert_eq!(turn_token_usage(&json!({})), json!({"available": false}));
        let usage = turn_token_usage(&json!({"input_tokens": 10, "output_tokens": 2}));
        assert_eq!(usage["available"], true);
        assert_eq!(usage["total_tokens"], 12);
    }

    #[test]
    fn session_token_totals_accumulate_across_turns() {
        let directory = std::env::temp_dir().join(format!(
            "tally-cursor-token-totals-{}-{}",
            std::process::id(),
            random_hex(4)
        ));
        let first = update_session_token_totals(
            &directory,
            "session-a",
            &json!({"available": true, "input_tokens": 10, "output_tokens": 2, "cache_read_tokens": 0, "cache_write_tokens": 0}),
        )
        .unwrap();
        assert_eq!(first["total_tokens"], 12);
        let second = update_session_token_totals(
            &directory,
            "session-a",
            &json!({"available": true, "input_tokens": 5, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0}),
        )
        .unwrap();
        assert_eq!(second["total_tokens"], 18);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
