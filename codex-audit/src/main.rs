use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const EVENTS: &[HookEvent] = &[
    HookEvent::new(
        "SessionStart",
        Some("*"),
        "Tally: recording Codex session start",
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
    HookEvent::new("Stop", None, "Tally: recording Codex stop"),
];

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("tally-codex: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("hook") => {
            let event = args
                .next()
                .or_else(|| env::var("CODEX_HOOK_EVENT").ok())
                .unwrap_or_else(|| "unknown".to_string());
            record_hook_event(&event)?;
            Ok(0)
        }
        Some("heartbeat-daemon" | "daemon") => {
            run_heartbeat_daemon()?;
            Ok(0)
        }
        Some("install-desktop-hooks" | "install") => {
            install_desktop_hooks()?;
            Ok(0)
        }
        Some("uninstall-desktop-hooks" | "uninstall") => {
            uninstall_desktop_hooks()?;
            Ok(0)
        }
        Some("wrap") => wrap_codex(args.collect()),
        Some(event_name) => {
            record_hook_event(event_name)?;
            Ok(0)
        }
        None => wrap_codex(Vec::new()),
    }
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

    let sink = AuditSink::new("codex-hooks")?;
    let raw_ref = sink.private_payload(
        &format!("hook_{}_{}", event_type, unique_suffix()),
        &payload,
    )?;
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
        "schema_version": "tally-codex.v1",
        "event_id": event_id,
        "run_id": sink.run_id,
        "source": "codex-hooks",
        "event_type": event_type,
        "observed_at": observed_at,
        "payload_hash": raw_ref["hash"],
        "payload_uri": raw_ref["uri"],
        "metadata": metadata,
    });

    sink.append_jsonl("codex-hooks", &event)?;
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

fn wrap_codex(args: Vec<String>) -> Result<i32> {
    set_runtime_defaults();

    let mut codex_args = Vec::new();
    if should_bypass_hook_trust(args.first().map(String::as_str)) {
        codex_args.push("--dangerously-bypass-hook-trust".to_string());
    }
    codex_args.extend(args.clone());

    if args.first().map(String::as_str) == Some("exec")
        && env_enabled("TALLY_TEE_CODEX_STDIO", true)
    {
        run_codex_with_tee(&codex_args)
    } else {
        let status = Command::new("codex").args(&codex_args).status()?;
        Ok(status.code().unwrap_or(1))
    }
}

fn should_bypass_hook_trust(first_arg: Option<&str>) -> bool {
    if !env_enabled("TALLY_BYPASS_HOOK_TRUST", true) {
        return false;
    }
    !matches!(
        first_arg,
        Some("login" | "logout" | "help" | "--help" | "-h" | "--version" | "version")
    )
}

fn run_codex_with_tee(args: &[String]) -> Result<i32> {
    let stdio_dir = log_root().join("codex-stdio");
    fs::create_dir_all(&stdio_dir)?;
    let run_id = run_id();
    let stdout_log = stdio_dir.join(format!("{run_id}.stdout.log"));
    let stderr_log = stdio_dir.join(format!("{run_id}.stderr.log"));

    let mut child = Command::new("codex")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or("failed to capture codex stdout")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("failed to capture codex stderr")?;
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
    if !env_enabled("TALLY_HOOK_HEARTBEAT_ENABLED", true) {
        return Ok(());
    }

    let session_id = extract_session_id(payload).unwrap_or_else(|| sink.run_id.clone());
    let state_path = heartbeat_state_path(&sink.run_id);
    let pid_path = heartbeat_pid_path(&sink.run_id);
    write_json_atomic(
        &state_path,
        &json!({
            "run_id": sink.run_id,
            "session_id": session_id,
            "updated_at": observed_at,
            "last_hook_event": event_type,
            "stop_requested": event_type == "Stop",
        }),
    )?;

    AuditSink::new("hook-heartbeat")?.emit_heartbeat(
        &[session_id],
        "hook-heartbeat",
        json!({"heartbeat_kind": "hook-event", "hook_event": event_type}),
    )?;

    if event_type != "SessionStart" {
        return Ok(());
    }
    if pid_path.exists() {
        let pid = fs::read_to_string(&pid_path).unwrap_or_default();
        if process_is_alive(pid.trim()) {
            return Ok(());
        }
        let _ = fs::remove_file(&pid_path);
    }

    Command::new(env::current_exe()?)
        .arg("heartbeat-daemon")
        .env("TALLY_RUN_ID", &sink.run_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

fn run_heartbeat_daemon() -> Result<()> {
    set_runtime_defaults();

    let sink = AuditSink::new("hook-heartbeat")?;
    let state_path = heartbeat_state_path(&sink.run_id);
    let pid_path = heartbeat_pid_path(&sink.run_id);
    fs::write(&pid_path, std::process::id().to_string())?;
    write_json_atomic(
        &heartbeat_daemon_status_path(&sink.run_id),
        &json!({
            "pid": std::process::id(),
            "status": "started",
            "updated_at": utc_now(),
            "state_path": state_path,
        }),
    )?;

    let interval = env_u64(
        "TALLY_HOOK_HEARTBEAT_SECONDS",
        env_u64("TALLY_HEARTBEAT_SECONDS", 60),
    );
    let idle_timeout = env_u64("TALLY_HOOK_HEARTBEAT_IDLE_SECONDS", 300) as i64;

    loop {
        let state = read_json_file(&state_path).unwrap_or_else(|_| json!({}));
        if state["stop_requested"].as_bool().unwrap_or(false) {
            break;
        }
        let last_update = state["updated_at"].as_str().unwrap_or("");
        if seconds_since(last_update) > idle_timeout {
            sink.emit_heartbeat(
                &[state["session_id"]
                    .as_str()
                    .unwrap_or(&sink.run_id)
                    .to_string()],
                "hook-heartbeat",
                json!({
                    "heartbeat_kind": "hook-daemon-timeout",
                    "last_hook_event": state["last_hook_event"],
                    "last_hook_observed_at": state["updated_at"],
                }),
            )?;
            break;
        }
        sink.emit_heartbeat(
            &[state["session_id"]
                .as_str()
                .unwrap_or(&sink.run_id)
                .to_string()],
            "hook-heartbeat",
            json!({
                "heartbeat_kind": "hook-daemon",
                "last_hook_event": state["last_hook_event"],
                "last_hook_observed_at": state["updated_at"],
            }),
        )?;
        thread::sleep(Duration::from_secs(interval));
    }

    write_json_atomic(
        &heartbeat_daemon_status_path(&sink.run_id),
        &json!({
            "pid": std::process::id(),
            "status": "stopped",
            "updated_at": utc_now(),
            "state_path": state_path,
        }),
    )?;
    let _ = fs::remove_file(pid_path);
    Ok(())
}

fn install_desktop_hooks() -> Result<()> {
    set_runtime_defaults();

    let hooks_path = hooks_path();
    fs::create_dir_all(hooks_path.parent().unwrap_or_else(|| Path::new(".")))?;
    fs::create_dir_all(log_root())?;
    let hook_bin = env::current_exe()?.display().to_string();

    let mut config = if hooks_path.exists() {
        read_json_file(&hooks_path)?
    } else {
        json!({"hooks": {}})
    };
    if !config.is_object() {
        return Err(format!(
            "refusing to modify non-object JSON at {}",
            hooks_path.display()
        )
        .into());
    }
    if !config.get("hooks").map(Value::is_object).unwrap_or(false) {
        config["hooks"] = json!({});
    }

    let backup = backup_if_exists(&hooks_path)?;
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
            .push(event.to_hook_group(&hook_bin));
    }

    write_json_pretty(&hooks_path, &config)?;
    println!(
        "Installed Tally Codex Desktop hooks into {}",
        hooks_path.display()
    );
    if let Some(backup) = backup {
        println!("Backed up previous hooks file to {}", backup.display());
    }
    println!("Hook binary: {hook_bin}");
    println!("Logs: {}", log_root().display());
    Ok(())
}

fn uninstall_desktop_hooks() -> Result<()> {
    let hooks_path = hooks_path();
    if !hooks_path.exists() {
        println!("No hooks file found at {}", hooks_path.display());
        return Ok(());
    }
    let mut config = read_json_file(&hooks_path)?;
    if !config.is_object() || !config.get("hooks").map(Value::is_object).unwrap_or(false) {
        return Err(format!(
            "refusing to modify {}: unexpected hooks file shape",
            hooks_path.display()
        )
        .into());
    }
    let backup = backup_if_exists(&hooks_path)?;
    let removed = remove_tally_hooks(&mut config);
    write_json_pretty(&hooks_path, &config)?;
    println!(
        "Removed {removed} Tally hook handler(s) from {}",
        hooks_path.display()
    );
    if let Some(backup) = backup {
        println!("Backed up previous hooks file to {}", backup.display());
    }
    Ok(())
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
            "schema_version": "0.2-mvp",
            "session_id": session_id,
            "agent_id": agent_id(),
            "agent_version": agent_version(),
            "principal": {"type": "human", "id": "[ARB] codex-user"},
            "authority_scope_hash": sha256_value(metadata),
            "authority_scope_uri": raw_uri,
            "authority_granted_at": observed_at,
            "session_started_at": metadata["observed_at"],
            "codex_hook_event": event_type,
            "raw_hook_hash": raw_hash,
        }),
        "UserPromptSubmit" => {
            let summary = prompt
                .map(|value| value.chars().take(240).collect::<String>())
                .unwrap_or_else(|| "User prompt submitted to Codex".to_string());
            json!({
                "record_type": "INSTRUCTION_RECEIVED",
                "schema_version": "0.2-mvp",
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
                "codex_hook_event": event_type,
            })
        }
        "PreToolUse" | "PermissionRequest" => {
            let tool_params =
                first_mapping_by_key(payload, &["arguments", "args", "params", "input"])
                    .cloned()
                    .unwrap_or_else(|| payload.clone());
            json!({
                "record_type": "ACTION_TAKEN",
                "schema_version": "0.2-mvp",
                "session_id": session_id,
                "action_id": action_id(payload),
                "instruction_id": first_string_by_key(payload, &["instruction_id", "turn_id", "turnId"])
                    .unwrap_or_else(|| stable_id("instr", &Value::String(session_id.clone()))),
                "action_type": if event_type == "PermissionRequest" { "decision" } else { "tool_call" },
                "tool": {
                    "server": first_string_by_key(payload, &["server", "server_name", "mcp_server", "recipient_namespace"])
                        .unwrap_or_else(|| "codex".to_string()),
                    "name": first_string_by_key(payload, &["tool_name", "toolName", "name", "command", "mcp_tool_name", "recipient_name"])
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
                "codex_hook_event": event_type,
                "raw_hook_hash": raw_ref["hash"],
            })
        }
        "PostToolUse" => {
            let has_error = first_string_by_key(payload, &["error", "exception"]).is_some();
            json!({
                "record_type": "RESULT_RECEIVED",
                "schema_version": "0.2-mvp",
                "session_id": session_id,
                "action_id": action_id(payload),
                "result_hash": raw_ref["hash"],
                "result_uri": raw_ref["uri"],
                "result_received_at": observed_at,
                "result_interpretation": {
                    "summary": "[ARB] Codex reported a tool result",
                    "detail_hash": raw_ref["hash"],
                    "detail_uri": raw_ref["uri"],
                },
                "exception": {
                    "occurred": has_error,
                    "type": first_string_by_key(payload, &["error_type", "type"]),
                    "description_hash": if has_error { raw_ref["hash"].clone() } else { Value::Null },
                    "description_uri": if has_error { raw_ref["uri"].clone() } else { Value::Null },
                },
                "codex_hook_event": event_type,
            })
        }
        "Stop" => json!({
            "record_type": "SESSION_END",
            "schema_version": "0.2-mvp",
            "session_id": session_id,
            "outcome": "codex_turn_stopped",
            "outcome_hash": raw_ref["hash"],
            "outcome_uri": raw_ref["uri"],
            "session_ended_at": observed_at,
            "codex_hook_event": event_type,
        }),
        _ => json!({
            "record_type": "CODEX_LIFECYCLE",
            "schema_version": "0.2-mvp",
            "session_id": session_id,
            "codex_hook_event": event_type,
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
        "Stop" => "SESSION_END",
        _ => "CODEX_LIFECYCLE",
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

    fn to_hook_group(self, hook_bin: &str) -> Value {
        let mut group = serde_json::Map::new();
        if let Some(matcher) = self.matcher {
            group.insert("matcher".to_string(), Value::String(matcher.to_string()));
        }
        group.insert(
            "hooks".to_string(),
            json!([{
                "type": "command",
                "command": format!("{} hook {}", shell_quote(hook_bin), shell_quote(self.name)),
                "timeout": 15,
                "statusMessage": self.status,
            }]),
        );
        Value::Object(group)
    }
}

struct AuditSink {
    source: String,
    run_id: String,
    workspace: PathBuf,
    jsonl_dir: PathBuf,
    tally_dir: PathBuf,
    private_dir: PathBuf,
    state_dir: PathBuf,
}

impl AuditSink {
    fn new(source: &str) -> Result<Self> {
        let source = safe_slug(source, "source");
        let root = log_root();
        let run_id = run_id();
        let workspace = workspace_path();
        let jsonl_dir = root.join("jsonl");
        let tally_dir = root.join("tally").join(&source);
        let private_dir = root.join("private").join(&run_id).join(&source);
        let state_dir = root.join("state");
        for path in [&jsonl_dir, &tally_dir, &private_dir, &state_dir] {
            fs::create_dir_all(path)?;
        }
        Ok(Self {
            source,
            run_id,
            workspace,
            jsonl_dir,
            tally_dir,
            private_dir,
            state_dir,
        })
    }

    fn next_sequence(&self) -> Result<u64> {
        let counter = self.state_dir.join(format!("{}.counter", self.source));
        let lock_path = self.state_dir.join(format!("{}.counter.lock", self.source));
        let lock = OpenOptions::new()
            .create(true)
            .append(true)
            .open(lock_path)?;
        lock.lock_exclusive()?;
        let current = fs::read_to_string(&counter)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let next = current + 1;
        fs::write(counter, next.to_string())?;
        lock.unlock()?;
        Ok(next)
    }

    fn private_payload(&self, label: &str, payload: &Value) -> Result<Value> {
        let path = self
            .private_dir
            .join(format!("{}.json", safe_slug(label, "payload")));
        write_json_atomic(&path, payload)?;
        Ok(json!({
            "hash": sha256_value(payload),
            "uri": format!("private://{}/{}/{}", self.run_id, self.source, path.file_name().unwrap().to_string_lossy()),
            "path": path.display().to_string(),
        }))
    }

    fn append_jsonl(&self, stream_name: &str, event: &Value) -> Result<()> {
        append_jsonl_locked(
            &self
                .jsonl_dir
                .join(format!("{}.jsonl", safe_slug(stream_name, "stream"))),
            event,
        )
    }

    fn write_tally_record(&self, record: &Value) -> Result<PathBuf> {
        let seq = self.next_sequence()?;
        let record_type = record["record_type"].as_str().unwrap_or("RECORD");
        let record_id = record["record_id"].as_str().unwrap_or("record");
        let path = self.tally_dir.join(format!(
            "{seq:06}_{}_{}.json",
            safe_slug(record_type, "RECORD"),
            safe_slug(record_id, "record")
        ));
        let mut with_defaults = record.clone();
        with_defaults["run_id"] = Value::String(self.run_id.clone());
        if with_defaults.get("schema_version").is_none() {
            with_defaults["schema_version"] = Value::String("0.2-mvp".to_string());
        }
        write_json_atomic(&path, &with_defaults)?;
        Ok(path)
    }

    fn emit_heartbeat(
        &self,
        active_sessions: &[String],
        stream_name: &str,
        extra: Value,
    ) -> Result<()> {
        let timestamp = utc_now();
        let mut event = json!({
            "schema_version": "tally-codex.v1",
            "run_id": self.run_id,
            "source": self.source,
            "event_type": "heartbeat",
            "observed_at": timestamp,
            "workspace": self.workspace.display().to_string(),
        });
        merge_object(&mut event, extra.clone());
        self.append_jsonl(stream_name, &event)?;
        self.write_tally_record(&json!({
            "record_type": "HEARTBEAT",
            "schema_version": "0.2-mvp",
            "session_id": self.run_id,
            "agent_id": agent_id(),
            "active_sessions": active_sessions,
            "timestamp": timestamp,
            "source": self.source,
            "metadata": extra,
        }))?;
        Ok(())
    }
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
                let is_current_hook = command.contains("tally-codex") && command.contains(" hook ");
                let is_legacy_hook =
                    command.contains("tally-host-hook") || command.contains("codex_hook_logger.py");
                !(is_current_hook || is_legacy_hook)
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
    set_default("TALLY_AGENT_ID", "codex-desktop");
    set_default("TALLY_AGENT_VERSION", "codex");
    set_default("TALLY_HOOK_HEARTBEAT_SECONDS", "60");
}

fn set_default(key: &str, value: &str) {
    if env::var(key).unwrap_or_default().is_empty() {
        env::set_var(key, value);
    }
}

fn read_stdin() -> Result<String> {
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;
    Ok(raw)
}

fn parse_payload(raw: &str) -> Value {
    if raw.is_empty() {
        json!({})
    } else {
        serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!({"raw_stdin": raw}))
    }
}

fn first_string_by_key(value: &Value, names: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for name in names {
                if let Some(Value::String(value)) = map.get(*name) {
                    if !value.is_empty() {
                        return Some(value.clone());
                    }
                }
            }
            map.values()
                .find_map(|item| first_string_by_key(item, names))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|item| first_string_by_key(item, names)),
        _ => None,
    }
}

fn first_mapping_by_key<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            for name in names {
                if let Some(item @ Value::Object(_)) = map.get(*name) {
                    return Some(item);
                }
            }
            map.values()
                .find_map(|item| first_mapping_by_key(item, names))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|item| first_mapping_by_key(item, names)),
        _ => None,
    }
}

fn extract_session_id(payload: &Value) -> Option<String> {
    first_string_by_key(
        payload,
        &[
            "session_id",
            "thread_id",
            "conversation_id",
            "conversationId",
        ],
    )
}

fn derive_run_id(payload: &Value) -> Option<String> {
    extract_session_id(payload)
        .or_else(|| env::var("CODEX_THREAD_ID").ok())
        .map(|value| safe_slug(&format!("codex_{value}"), "codex-session"))
}

fn stable_id(prefix: &str, value: &Value) -> String {
    let digest = sha256_value(value);
    format!(
        "{}_{}",
        prefix,
        &digest["sha256:".len().."sha256:".len() + 16]
    )
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

fn scrub_environment() -> Value {
    let allowed: BTreeSet<&str> = [
        "CODEX_HOME",
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
        if allowed.contains(key.as_str()) || key.starts_with("TALLY_") || key.starts_with("CODEX_")
        {
            out.insert(key, Value::String(value.chars().take(500).collect()));
        }
    }
    Value::Object(out)
}

fn light_git_state(cwd: &Path) -> Value {
    if !cwd.join(".git").exists() {
        return json!({"is_git_repo": false, "workspace": cwd.display().to_string()});
    }
    let head = run_command(["git", "rev-parse", "--verify", "HEAD"], cwd);
    let branch = run_command(["git", "branch", "--show-current"], cwd);
    let status = run_command(["git", "status", "--short", "--branch"], cwd);
    let status_stdout = status["stdout"].as_str().unwrap_or("");
    json!({
        "is_git_repo": true,
        "workspace": cwd.display().to_string(),
        "head": head["stdout"].as_str().unwrap_or("").trim(),
        "branch": branch["stdout"].as_str().unwrap_or("").trim(),
        "status_hash": sha256_str(status_stdout),
        "status": tail_chars(status_stdout, 20_000),
    })
}

fn run_command<const N: usize>(argv: [&str; N], cwd: &Path) -> Value {
    let started = SystemTime::now();
    let argv_vec = argv.to_vec();
    match Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .output()
    {
        Ok(output) => json!({
            "argv": argv_vec,
            "exit_code": output.status.code(),
            "duration_ms": started.elapsed().map(|d| d.as_millis()).unwrap_or(0),
            "stdout": tail_chars(&String::from_utf8_lossy(&output.stdout), 20_000),
            "stderr": tail_chars(&String::from_utf8_lossy(&output.stderr), 20_000),
        }),
        Err(error) => json!({
            "argv": argv_vec,
            "exit_code": Value::Null,
            "duration_ms": started.elapsed().map(|d| d.as_millis()).unwrap_or(0),
            "error": error.to_string(),
        }),
    }
}

fn tail_chars(value: &str, max: usize) -> String {
    let chars: Vec<char> = value.chars().rev().take(max).collect();
    chars.into_iter().rev().collect()
}

fn append_jsonl_locked(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    let lock = OpenOptions::new()
        .create(true)
        .append(true)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", canonical_json(value)?)?;
    lock.unlock()?;
    Ok(())
}

fn read_json_file(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}-{}", std::process::id(), random_hex(4)));
    write_json_pretty(&tmp, value)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn write_json_pretty(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    Ok(())
}

fn canonical_json(value: &Value) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn sha256_value(value: &Value) -> String {
    sha256_bytes(canonical_json(value).unwrap_or_default().as_bytes())
}

fn sha256_str(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let bytes = digest.finalize();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("sha256:{hex}")
}

fn merge_object(target: &mut Value, extra: Value) {
    let Some(target) = target.as_object_mut() else {
        return;
    };
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            target.insert(key, value);
        }
    }
}

fn safe_slug(value: &str, default: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= 96 {
            break;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        default.to_string()
    } else {
        out
    }
}

fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn backup_stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%S%.9fZ").to_string()
}

fn random_hex(bytes: usize) -> String {
    let mut input = Vec::new();
    input.extend_from_slice(
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    input.extend_from_slice(&std::process::id().to_le_bytes());
    input.extend_from_slice(format!("{:?}", thread::current().id()).as_bytes());
    let hash = sha256_bytes(&input);
    hash["sha256:".len().."sha256:".len() + bytes * 2].to_string()
}

fn seconds_since(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| (Utc::now() - dt.with_timezone(&Utc)).num_seconds())
        .unwrap_or(0)
}

fn process_is_alive(pid: &str) -> bool {
    if pid.is_empty() {
        return false;
    }
    Command::new("kill")
        .arg("-0")
        .arg(pid)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn unique_suffix() -> String {
    format!("{}-{}", std::process::id(), random_hex(4))
}

fn home_dir() -> String {
    env::var("HOME").unwrap_or_else(|_| ".".to_string())
}

fn default_log_root() -> String {
    format!("{}/.tally-codex/logs", home_dir())
}

fn log_root() -> PathBuf {
    expand_home(&env::var("TALLY_LOG_ROOT").unwrap_or_else(|_| default_log_root()))
}

fn workspace_path() -> PathBuf {
    expand_home(
        &env::var("TALLY_WORKSPACE")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .display()
                    .to_string()
            }),
    )
}

fn run_id() -> String {
    env::var("TALLY_RUN_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| safe_slug(&value, "run"))
        .unwrap_or_else(|| {
            safe_slug(
                &format!(
                    "run_{}",
                    env::var("USER").unwrap_or_else(|_| "local".to_string())
                ),
                "run",
            )
        })
}

fn hooks_path() -> PathBuf {
    if let Ok(path) = env::var("CODEX_HOOKS_PATH") {
        return expand_home(&path);
    }
    let codex_home = env::var("CODEX_HOME").unwrap_or_else(|_| format!("{}/.codex", home_dir()));
    expand_home(&format!("{codex_home}/hooks.json"))
}

fn expand_home(value: &str) -> PathBuf {
    if value == "~" {
        PathBuf::from(home_dir())
    } else if let Some(rest) = value.strip_prefix("~/") {
        PathBuf::from(home_dir()).join(rest)
    } else {
        PathBuf::from(value)
    }
}

fn heartbeat_state_path(run_id: &str) -> PathBuf {
    log_root()
        .join("state")
        .join(format!("hook-heartbeat.{}.json", safe_slug(run_id, "run")))
}

fn heartbeat_pid_path(run_id: &str) -> PathBuf {
    log_root()
        .join("state")
        .join(format!("hook-heartbeat.{}.pid", safe_slug(run_id, "run")))
}

fn heartbeat_daemon_status_path(run_id: &str) -> PathBuf {
    log_root().join("state").join(format!(
        "hook-heartbeat.{}.daemon.json",
        safe_slug(run_id, "run")
    ))
}

fn backup_if_exists(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let backup = path.with_file_name(format!(
        "{}.backup-{}",
        path.file_name().unwrap().to_string_lossy(),
        backup_stamp()
    ));
    fs::copy(path, &backup)?;
    Ok(Some(backup))
}

fn env_enabled(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(value) => !matches!(value.as_str(), "0" | "false" | "False" | "no"),
        Err(_) => default,
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn agent_id() -> String {
    env::var("TALLY_AGENT_ID").unwrap_or_else(|_| "codex-desktop".to_string())
}

fn agent_version() -> String {
    env::var("TALLY_AGENT_VERSION").unwrap_or_else(|_| "codex".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_payload_keeps_json_and_wraps_raw_text() {
        assert_eq!(parse_payload(r#"{"thread_id":"t1"}"#)["thread_id"], "t1");
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
        assert_eq!(record_type_for_hook("Stop"), "SESSION_END");
        assert_eq!(record_type_for_hook("Other"), "CODEX_LIFECYCLE");
    }

    #[test]
    fn uses_tool_call_id_to_correlate_actions_and_results() {
        let pre = json!({"tool_call_id": "tool-123", "arguments": {"command": "true"}});
        let post = json!({"tool_call_id": "tool-123", "result": {"stdout": ""}});
        assert_eq!(action_id(&pre), "act_tool-123");
        assert_eq!(action_id(&pre), action_id(&post));
    }

    #[test]
    fn removes_current_and_legacy_tally_hooks_only() {
        let mut config = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [
                        {"type": "command", "command": "/bin/echo keep"},
                        {"type": "command", "command": "/tmp/tally-codex hook SessionStart"},
                        {"type": "command", "command": "/tmp/tally-host-hook SessionStart"},
                        {"type": "command", "command": "python3 codex_hook_logger.py SessionStart"}
                    ]
                }]
            }
        });
        assert_eq!(remove_tally_hooks(&mut config), 3);
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
