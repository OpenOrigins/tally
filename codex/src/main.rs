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
use toml_edit::{Array as TomlArray, DocumentMut, Item as TomlItem, Value as TomlValue};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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
    HookEvent::new("Stop", None, "Tally: recording Codex turn end"),
    HookEvent::new("SessionEnd", None, "Tally: recording Codex session end"),
];

pub fn dispatch(arguments: Vec<String>) -> Result<i32> {
    let mut args = arguments.into_iter();
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
        Some("forward-pending") => {
            tally_common::forward_pending(&onboarding_state_dir())?;
            Ok(0)
        }
        Some("notify") => {
            handle_desktop_notification(args.collect())?;
            Ok(0)
        }
        Some("install-desktop-hooks" | "install") => {
            let options = tally_common::parse_install_options(args.collect::<Vec<_>>(), "Codex")?;
            install_desktop_hooks(options)?;
            Ok(0)
        }
        Some("uninstall-desktop-hooks" | "uninstall") => {
            let config_path = tally_common::parse_config_path_options(args.collect::<Vec<_>>())?;
            uninstall_desktop_hooks(config_path)?;
            Ok(0)
        }
        Some("wrap") => wrap_codex(args.collect()),
        Some("--help" | "-h" | "help") => {
            print_help();
            Ok(0)
        }
        Some("--version" | "version") => {
            println!("tally-codex {}", env!("CARGO_PKG_VERSION"));
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
        "tally-codex {}\n\nCommands:\n  gui           Open the graphical installer\n  install --api-key <KEY> [--api-url <URL>] [--config-path <PATH>]\n                Install or update Codex hooks\n  uninstall [--config-path <PATH>]\n                Remove Tally hooks and local credentials\n  wrap [ARGS]   Run Codex through Tally\n  hook EVENT    Record a hook event\n  notify        Record a Codex Desktop turn notification\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn record_hook_event(event_type: &str) -> Result<()> {
    let raw = read_stdin()?;
    let payload = parse_payload(&raw);
    record_payload_event(event_type, &raw, &payload, "codex-hooks", true)?;
    if event_type == "Stop" {
        mark_turn_complete(&onboarding_state_dir(), "hook", &payload)?;
    }
    Ok(())
}

fn record_payload_event(
    event_type: &str,
    raw: &str,
    payload: &Value,
    source: &str,
    update_heartbeat: bool,
) -> Result<()> {
    set_runtime_defaults();
    if env::var("TALLY_RUN_ID").unwrap_or_default().is_empty() {
        if let Some(run_id) = derive_run_id(payload) {
            env::set_var("TALLY_RUN_ID", run_id);
        }
    }

    let sink = AuditSink::new(source)?;
    let raw_ref =
        sink.private_payload(&format!("hook_{}_{}", event_type, unique_suffix()), payload)?;
    let observed_at = utc_now();
    let metadata = json!({
        "observed_at": observed_at,
        "hook_event": event_type,
        "cwd": env::current_dir()?.display().to_string(),
        "argv": scrub_argv(),
        "raw_stdin_hash": sha256_str(raw),
        "environment": scrub_environment(),
        "git_state": light_git_state(&workspace_path()),
    });
    let event_id = format!("evt_{}", random_hex(8));
    let event = json!({
        "schema_version": "tally-codex.v1",
        "event_id": event_id,
        "run_id": sink.run_id,
        "source": source,
        "event_type": event_type,
        "observed_at": observed_at,
        "payload_hash": raw_ref["hash"],
        "payload_uri": raw_ref["uri"],
        "metadata": metadata,
    });

    sink.append_jsonl(source, &event)?;
    if update_heartbeat {
        update_heartbeat_state(
            &sink,
            event_type,
            payload,
            event["observed_at"].as_str().unwrap_or(&utc_now()),
        )?;
    }

    let mut record = build_tally_record(&sink, event_type, payload, &raw_ref, &metadata);
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

fn handle_desktop_notification(arguments: Vec<String>) -> Result<()> {
    let mut state_dir = None;
    let mut raw = None;
    let mut args = arguments.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--state-dir" => {
                state_dir = Some(PathBuf::from(
                    args.next().ok_or("--state-dir requires a value")?,
                ));
            }
            _ if argument.starts_with("--state-dir=") => {
                state_dir = Some(PathBuf::from(&argument["--state-dir=".len()..]));
            }
            _ if raw.is_none() => raw = Some(argument),
            _ => return Err("notify accepts exactly one JSON payload".into()),
        }
    }
    if let Some(state_dir) = state_dir {
        env::set_var("TALLY_STATE_DIR", state_dir);
    }
    let raw = raw.ok_or("notify requires the JSON payload supplied by Codex")?;
    let result = record_desktop_turn(&raw);
    if let Err(error) = run_previous_notify(&raw) {
        eprintln!("Warning: the previous Codex notification command failed: {error}");
    }
    result
}

fn record_desktop_turn(raw: &str) -> Result<()> {
    let payload = parse_payload(raw);
    if payload["type"].as_str() != Some("agent-turn-complete") {
        return Ok(());
    }
    let session_id = first_string_by_key(&payload, &["thread-id"])
        .ok_or("Codex notification is missing thread-id")?;
    let turn_id = first_string_by_key(&payload, &["turn-id"])
        .ok_or("Codex notification is missing turn-id")?;
    env::set_var(
        "TALLY_RUN_ID",
        safe_slug(&format!("codex_{session_id}"), "codex-session"),
    );

    let state_dir = onboarding_state_dir();
    let marker_dir = state_dir.join("desktop-notifications");
    fs::create_dir_all(&marker_dir)?;
    let marker_key = stable_id("turn", &json!([session_id, turn_id]));
    let lock_path = marker_dir.join(format!("{marker_key}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    lock.lock_exclusive()?;

    let completed_path = marker_dir.join(format!("{marker_key}.json"));
    if completed_path.exists()
        || turn_marker_path(&state_dir, "hook", &payload).is_some_and(|p| p.exists())
    {
        FileExt::unlock(&lock)?;
        return Ok(());
    }

    let base = json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "cwd": payload["cwd"],
        "client": payload["client"],
        "desktop_notification": true,
    });
    let session_marker = marker_dir.join(format!(
        "{}.session.json",
        stable_id("session", &Value::String(session_id.clone()))
    ));
    if !session_marker.exists() {
        record_payload_event("SessionStart", raw, &base, "codex-desktop", false)?;
        write_json_atomic(
            &session_marker,
            &json!({"session_id": session_id, "observed_at": utc_now()}),
        )?;
    }

    let prompt = payload["input-messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let mut instruction = base.clone();
    instruction["prompt"] = Value::String(prompt);
    record_payload_event(
        "UserPromptSubmit",
        raw,
        &instruction,
        "codex-desktop",
        false,
    )?;

    let mut turn_end = base;
    turn_end["last_assistant_message"] = payload["last-assistant-message"].clone();
    record_payload_event("Stop", raw, &turn_end, "codex-desktop", false)?;
    write_json_atomic(
        &completed_path,
        &json!({
            "session_id": session_id,
            "turn_id": turn_id,
            "observed_at": utc_now(),
        }),
    )?;
    FileExt::unlock(&lock)?;
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
            "stop_requested": heartbeat_stop_requested(event_type),
        }),
    )?;
    tally_common::record_agent_activity(&sink.state_dir, &agent_id(), unix_now_millis())?;

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

    tally_common::spawn_background(&env::current_exe()?, &["codex", "heartbeat-daemon"])?;
    Ok(())
}

fn run_heartbeat_daemon() -> Result<()> {
    set_runtime_defaults();

    let sink = AuditSink::new("hook-heartbeat")?;
    let state_path = heartbeat_state_path(&sink.run_id);
    let pid_path = heartbeat_pid_path(&sink.run_id);
    let Some(pid_file) = claim_heartbeat_daemon(&pid_path)? else {
        return Ok(());
    };
    write_json_atomic(
        &heartbeat_daemon_status_path(&sink.run_id),
        &json!({
            "pid": std::process::id(),
            "status": "started",
            "updated_at": utc_now(),
            "state_path": state_path,
        }),
    )?;

    let interval = configured_heartbeat_interval();
    let poll_interval =
        env_u64("TALLY_HOOK_HEARTBEAT_POLL_SECONDS", interval.min(30)).clamp(1, interval);
    let idle_timeout = env_u64(
        "TALLY_HOOK_HEARTBEAT_IDLE_SECONDS",
        interval.saturating_mul(3),
    )
    .max(interval)
    .min(i64::MAX as u64) as i64;

    loop {
        thread::sleep(Duration::from_secs(poll_interval));
        let state = read_json_file(&state_path).unwrap_or_else(|_| json!({}));
        if state["stop_requested"].as_bool().unwrap_or(false) {
            break;
        }
        let last_update = state["updated_at"].as_str().unwrap_or("");
        let quiet_seconds = seconds_since(last_update);
        if quiet_seconds > idle_timeout {
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
                interval,
            )?;
            break;
        }
        if !heartbeat_due(quiet_seconds, interval) {
            continue;
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
            interval,
        )?;
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
    FileExt::unlock(&pid_file)?;
    let _ = fs::remove_file(pid_path);
    Ok(())
}

fn claim_heartbeat_daemon(path: &Path) -> io::Result<Option<fs::File>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = match OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if heartbeat_lock_is_contended(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if heartbeat_lock_is_contended(&error) => return Ok(None),
        Err(error) => return Err(error),
    }
    file.set_len(0)?;
    file.write_all(std::process::id().to_string().as_bytes())?;
    file.sync_all()?;
    Ok(Some(file))
}

fn heartbeat_lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32) | Some(33)) {
        return true;
    }
    false
}

fn heartbeat_due(quiet_seconds: i64, interval_seconds: u64) -> bool {
    quiet_seconds >= 0 && quiet_seconds as u64 >= interval_seconds
}

fn heartbeat_stop_requested(event_type: &str) -> bool {
    event_type == "SessionEnd"
}

fn configured_heartbeat_interval() -> u64 {
    tally_common::heartbeat_interval_seconds(env_u64(
        "TALLY_HOOK_HEARTBEAT_SECONDS",
        env_u64(
            "TALLY_HEARTBEAT_SECONDS",
            tally_common::DEFAULT_HEARTBEAT_INTERVAL_SECONDS,
        ),
    ))
}

pub fn install_desktop_hooks(
    options: tally_common::InstallOptions,
) -> Result<tally_common::InstallReport> {
    set_runtime_defaults();

    let hooks_path = effective_hooks_path(options.config_path.as_deref());
    let state_dir = state_dir_for_hooks_path(&hooks_path);
    let installed_binary_path = installed_binary_path_for_hooks_path(&hooks_path);
    let codex_config_path = codex_config_path_for_hooks_path(&hooks_path);
    let previous_notify_path = previous_notify_path(&state_dir);
    fs::create_dir_all(hooks_path.parent().unwrap_or_else(|| Path::new(".")))?;
    tally_common::mark_tally_data_directory(&log_root())?;
    let source_binary = tally_common::installation_source_executable()?;
    let hook_bin = installed_binary_path.display().to_string();

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
    let codex_config_backup = backup_if_exists(&codex_config_path)?;
    let hooks_snapshot = tally_common::FileSnapshot::capture(&hooks_path)?;
    let codex_config_snapshot = tally_common::FileSnapshot::capture(&codex_config_path)?;
    let previous_notify_snapshot = tally_common::FileSnapshot::capture(&previous_notify_path)?;
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
        write_json_atomic(&hooks_path, &config)?;
        install_desktop_notify(
            &codex_config_path,
            &installed_binary_path,
            &state_dir,
            &previous_notify_path,
        )?;
        if let Err(error) =
            tally_common::remove_legacy_installed_executable(&hooks_path, "tally-codex")
        {
            eprintln!("Warning: could not remove the previous hook executable: {error}");
        }
        Ok(())
    })();
    if let Err(error) = install_result {
        let _ = hooks_snapshot.restore();
        let _ = codex_config_snapshot.restore();
        let _ = previous_notify_snapshot.restore();
        let _ = key_snapshot.restore();
        let _ = api_config_snapshot.restore();
        let _ = binary_snapshot.restore();
        return Err(error);
    }
    println!(
        "Installed Tally Codex Desktop hooks into {}",
        hooks_path.display()
    );
    if let Some(backup) = backup.as_ref() {
        println!("Backed up previous hooks file to {}", backup.display());
    }
    if let Some(backup) = codex_config_backup.as_ref() {
        println!("Backed up previous Codex config to {}", backup.display());
    }
    println!("Hook binary: {hook_bin}");
    println!(
        "Codex Desktop notifications: {}",
        codex_config_path.display()
    );
    println!("Logs: {}", log_root().display());
    println!(
        "Agent API key: stored securely at {}",
        tally_common::api_key_path(&state_dir).display()
    );
    println!("Ingest API: {}", options.api_url);
    let handshake_error =
        match tally_common::notify_client_connected(&options.api_key, &options.api_url, "codex") {
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
        config_path: hooks_path,
        state_dir,
        logs_path: log_root(),
        installed_binary_path,
        backup_path: backup,
        handshake_error,
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
    let hooks_path = effective_hooks_path(config_path.as_deref());
    let state_dir = state_dir_for_hooks_path(&hooks_path);
    let logs_path = log_root();
    uninstall_desktop_notify(
        &codex_config_path_for_hooks_path(&hooks_path),
        &installed_binary_path_for_hooks_path(&hooks_path),
        &state_dir,
        &previous_notify_path(&state_dir),
    )?;
    if hooks_path.exists() {
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
        write_json_atomic(&hooks_path, &config)?;
        println!(
            "Removed {removed} Tally hook handler(s) from {}",
            hooks_path.display()
        );
        if let Some(backup) = backup {
            println!("Backed up previous hooks file to {}", backup.display());
        }
    } else {
        println!("No hooks file found at {}", hooks_path.display());
    }
    remove_local_credentials_for_hooks_path(&hooks_path)?;
    if remove_data {
        tally_common::remove_tally_data(&state_dir, &logs_path)?;
    }
    Ok(tally_common::UninstallReport {
        config_path: hooks_path,
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
                "schema_version": "0.2",
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
                "schema_version": "0.2",
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
            "record_type": "TURN_END",
            "schema_version": "0.2",
            "session_id": session_id,
            "turn_id": turn_id(payload),
            "outcome": "completed",
            "outcome_hash": raw_ref["hash"],
            "outcome_uri": raw_ref["uri"],
            "turn_ended_at": observed_at,
            "codex_hook_event": event_type,
        }),
        "SessionEnd" => json!({
            "record_type": "SESSION_END",
            "schema_version": "0.2",
            "session_id": session_id,
            "outcome": "partial",
            "outcome_hash": raw_ref["hash"],
            "outcome_uri": raw_ref["uri"],
            "session_ended_at": observed_at,
            "session_end_reason": first_string_by_key(payload, &["reason"]),
            "codex_hook_event": event_type,
        }),
        _ => json!({
            "record_type": "CODEX_LIFECYCLE",
            "schema_version": "0.2",
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
        "Stop" => "TURN_END",
        "SessionEnd" => "SESSION_END",
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

    fn to_hook_group(self, hook_bin: &str, state_dir: &Path) -> Value {
        let mut group = serde_json::Map::new();
        if let Some(matcher) = self.matcher {
            group.insert("matcher".to_string(), Value::String(matcher.to_string()));
        }
        let command = hook_command(hook_bin, self.name, state_dir);
        let handler = json!({
            "type": "command",
            "command": command,
            "timeout": if self.name == "SessionEnd" { 3 } else { 15 },
            "statusMessage": self.status,
        });
        #[cfg(windows)]
        let handler = {
            let mut handler = handler;
            handler["commandWindows"] = handler["command"].clone();
            handler
        };
        group.insert("hooks".to_string(), Value::Array(vec![handler]));
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
        tally_common::mark_tally_data_directory(&root)?;
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
            .read(true)
            .write(true)
            .truncate(false)
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
            with_defaults["schema_version"] = Value::String("0.2".to_string());
        }
        write_json_atomic(&path, &with_defaults)?;
        let _ = tally_common::enqueue_record(&onboarding_state_dir(), &path, &env::current_exe()?);
        Ok(path)
    }

    fn emit_heartbeat(
        &self,
        active_sessions: &[String],
        stream_name: &str,
        mut metadata: Value,
        requested_interval_seconds: u64,
    ) -> Result<()> {
        let agent_id = agent_id();
        let emitted_at_unix_millis = unix_now_millis();
        let Some(rate_limit_seconds) = tally_common::claim_agent_heartbeat(
            &self.state_dir,
            &agent_id,
            emitted_at_unix_millis,
            requested_interval_seconds,
        )?
        else {
            return Ok(());
        };
        merge_object(
            &mut metadata,
            json!({"rate_limit_seconds": rate_limit_seconds}),
        );
        let timestamp = utc_now();
        let record_id = stable_id(
            "heartbeat",
            &json!({
                "agent_id": agent_id,
                "client": "codex",
                "emitted_at_unix_millis": emitted_at_unix_millis,
            }),
        );
        let mut event = json!({
            "schema_version": "tally-codex.v1",
            "record_id": record_id,
            "run_id": self.run_id,
            "source": self.source,
            "event_type": "heartbeat",
            "observed_at": timestamp,
            "workspace": self.workspace.display().to_string(),
        });
        merge_object(&mut event, metadata.clone());
        self.append_jsonl(stream_name, &event)?;
        self.write_tally_record(&json!({
            "record_type": "HEARTBEAT",
            "record_id": record_id,
            "schema_version": "0.2",
            "session_id": self.run_id,
            "agent_id": agent_id,
            "active_sessions": active_sessions,
            "timestamp": timestamp,
            "source": self.source,
            "metadata": metadata,
        }))?;
        Ok(())
    }
}

fn codex_config_path_for_hooks_path(hooks_path: &Path) -> PathBuf {
    hooks_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("config.toml")
}

fn previous_notify_path(state_dir: &Path) -> PathBuf {
    state_dir.join("previous-codex-notify.json")
}

fn tally_notify_command(binary_path: &Path, state_dir: &Path) -> Vec<String> {
    vec![
        binary_path.display().to_string(),
        "codex".to_string(),
        "notify".to_string(),
        "--state-dir".to_string(),
        state_dir.display().to_string(),
    ]
}

fn toml_notify_command(document: &DocumentMut) -> Result<Option<Vec<String>>> {
    let Some(item) = document.get("notify") else {
        return Ok(None);
    };
    let array = item
        .as_array()
        .ok_or("Codex config notify must be an array of command arguments")?;
    let mut command = Vec::with_capacity(array.len());
    for argument in array {
        command.push(
            argument
                .as_str()
                .ok_or("Codex config notify arguments must be strings")?
                .to_string(),
        );
    }
    if command.is_empty() {
        return Err("Codex config notify command cannot be empty".into());
    }
    Ok(Some(command))
}

fn set_toml_notify_command(document: &mut DocumentMut, command: &[String]) {
    let mut array = TomlArray::new();
    for argument in command {
        array.push(argument.as_str());
    }
    document["notify"] = TomlItem::Value(TomlValue::Array(array));
}

fn read_codex_config(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    Ok(fs::read_to_string(path)?.parse::<DocumentMut>()?)
}

fn install_desktop_notify(
    config_path: &Path,
    binary_path: &Path,
    state_dir: &Path,
    saved_notify_path: &Path,
) -> Result<()> {
    let config_existed = config_path.exists();
    let mut document = read_codex_config(config_path)?;
    let current = toml_notify_command(&document)?;
    let tally_command = tally_notify_command(binary_path, state_dir);

    if current.as_ref() != Some(&tally_command) {
        write_json_atomic(
            saved_notify_path,
            &json!({
                "config_existed": config_existed,
                "command": current,
            }),
        )?;
    } else if !saved_notify_path.exists() {
        write_json_atomic(
            saved_notify_path,
            &json!({"config_existed": config_existed, "command": Value::Null}),
        )?;
    }

    set_toml_notify_command(&mut document, &tally_command);
    write_text_atomic(config_path, &document.to_string())
}

fn uninstall_desktop_notify(
    config_path: &Path,
    binary_path: &Path,
    state_dir: &Path,
    saved_notify_path: &Path,
) -> Result<()> {
    if !config_path.exists() {
        remove_file_if_exists(saved_notify_path)?;
        return Ok(());
    }

    let mut document = read_codex_config(config_path)?;
    let current = toml_notify_command(&document)?;
    let tally_command = tally_notify_command(binary_path, state_dir);
    if current.as_ref() != Some(&tally_command) {
        remove_file_if_exists(saved_notify_path)?;
        return Ok(());
    }

    let saved = if saved_notify_path.exists() {
        read_json_file(saved_notify_path)?
    } else {
        json!({"config_existed": true, "command": Value::Null})
    };
    let previous = saved["command"].as_array().map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    if let Some(previous) = previous.filter(|command| !command.is_empty()) {
        set_toml_notify_command(&mut document, &previous);
    } else {
        document.remove("notify");
    }

    if !saved["config_existed"].as_bool().unwrap_or(true) && document.is_empty() {
        remove_file_if_exists(config_path)?;
    } else {
        write_text_atomic(config_path, &document.to_string())?;
    }
    remove_file_if_exists(saved_notify_path)?;
    Ok(())
}

fn run_previous_notify(raw: &str) -> Result<()> {
    let path = previous_notify_path(&onboarding_state_dir());
    if !path.exists() {
        return Ok(());
    }
    let saved = read_json_file(&path)?;
    let Some(command) = saved["command"].as_array() else {
        return Ok(());
    };
    let command = command
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or("saved Codex notify argument is not a string")
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let Some(program) = command.first() else {
        return Ok(());
    };
    let status = Command::new(program)
        .args(&command[1..])
        .arg(raw)
        .status()?;
    if !status.success() {
        return Err(format!("notification command exited with {status}").into());
    }
    Ok(())
}

fn turn_marker_path(state_dir: &Path, kind: &str, payload: &Value) -> Option<PathBuf> {
    let session_id = first_string_by_key(
        payload,
        &["session_id", "thread_id", "thread-id", "conversation_id"],
    )?;
    let turn_id = first_string_by_key(payload, &["turn_id", "turnId", "turn-id"])?;
    Some(state_dir.join("completed-turns").join(format!(
        "{}.{}.json",
        kind,
        stable_id("turn", &json!([session_id, turn_id]))
    )))
}

fn mark_turn_complete(state_dir: &Path, kind: &str, payload: &Value) -> Result<()> {
    let Some(path) = turn_marker_path(state_dir, kind, payload) else {
        return Ok(());
    };
    write_json_atomic(&path, &json!({"observed_at": utc_now()}))
}

fn write_text_atomic(path: &Path, value: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mode = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(path)
                .map(|metadata| metadata.permissions().mode() & 0o777)
                .unwrap_or(0o600)
        }
        #[cfg(not(unix))]
        {
            0o600
        }
    };
    let tmp = path.with_extension(format!("tmp-{}-{}", std::process::id(), random_hex(4)));
    fs::write(&tmp, value)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
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
                let is_current_hook = (command.contains("tally-codex")
                    || command.contains(" codex hook "))
                    && command.contains(" hook ");
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
    set_default(
        "TALLY_HOOK_HEARTBEAT_SECONDS",
        &tally_common::DEFAULT_HEARTBEAT_INTERVAL_SECONDS.to_string(),
    );
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
            "thread-id",
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

fn turn_id(payload: &Value) -> String {
    first_string_by_key(payload, &["turn_id", "turnId", "turn-id"])
        .unwrap_or_else(|| stable_id("turn", payload))
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

fn scrub_argv() -> Vec<String> {
    env::args()
        .map(|argument| {
            let is_notification_payload = serde_json::from_str::<Value>(&argument)
                .ok()
                .is_some_and(|value| value["type"].as_str() == Some("agent-turn-complete"));
            if is_notification_payload {
                "[REDACTED: Codex notification payload]".to_string()
            } else {
                argument
            }
        })
        .collect()
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
        .read(true)
        .write(true)
        .truncate(false)
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
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
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

fn unix_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
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

#[cfg(unix)]
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

#[cfg(windows)]
fn process_is_alive(pid: &str) -> bool {
    if pid.is_empty() || !pid.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .any(|field| field == pid)
        })
        .unwrap_or(false)
}

#[cfg(unix)]
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

#[cfg(windows)]
fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '\\' | '/' | '.' | '_' | '-' | ':'))
    {
        value.to_string()
    } else {
        format!("\"{value}\"")
    }
}

#[cfg(unix)]
fn hook_command(hook_bin: &str, event_name: &str, state_dir: &Path) -> String {
    format!(
        "TALLY_STATE_DIR={} {} codex hook {}",
        shell_quote(&state_dir.display().to_string()),
        shell_quote(hook_bin),
        shell_quote(event_name)
    )
}

#[cfg(windows)]
fn hook_command(hook_bin: &str, event_name: &str, state_dir: &Path) -> String {
    format!(
        "set \"TALLY_STATE_DIR={}\" && {} codex hook {}",
        state_dir.display(),
        shell_quote(hook_bin),
        shell_quote(event_name)
    )
}

fn unique_suffix() -> String {
    format!("{}-{}", std::process::id(), random_hex(4))
}

fn home_dir() -> String {
    env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .or_else(|_| {
            Ok::<_, env::VarError>(format!(
                "{}{}",
                env::var("HOMEDRIVE")?,
                env::var("HOMEPATH")?
            ))
        })
        .unwrap_or_else(|_| ".".to_string())
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

pub fn default_config_path() -> PathBuf {
    if let Ok(path) = env::var("CODEX_HOOKS_PATH") {
        return expand_home(&path);
    }
    let codex_home = env::var("CODEX_HOME").unwrap_or_else(|_| format!("{}/.codex", home_dir()));
    expand_home(&format!("{codex_home}/hooks.json"))
}

fn effective_hooks_path(config_path: Option<&Path>) -> PathBuf {
    config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_path)
}

fn onboarding_state_dir() -> PathBuf {
    if let Ok(path) = env::var("TALLY_STATE_DIR") {
        return expand_home(&path);
    }
    state_dir_for_hooks_path(&default_config_path())
}

fn state_dir_for_hooks_path(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tally")
        .join("logs")
        .join(".state")
}

pub fn default_state_dir() -> PathBuf {
    state_dir_for_hooks_path(&default_config_path())
}

pub fn default_installed_binary_path() -> PathBuf {
    installed_binary_path_for_hooks_path(&default_config_path())
}

fn installed_binary_path_for_hooks_path(path: &Path) -> PathBuf {
    tally_common::installed_executable_path(path, "tally-codex")
}

fn remove_local_credentials_for_hooks_path(path: &Path) -> Result<()> {
    let state_dir = state_dir_for_hooks_path(path);
    for path in [
        tally_common::api_key_path(&state_dir),
        tally_common::config_path(&state_dir),
        previous_notify_path(&state_dir),
        installed_binary_path_for_hooks_path(path),
        tally_common::legacy_installed_executable_path(path, "tally-codex"),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
        let notification = json!({"thread-id": "thread-123", "turn-id": "turn-123"});
        assert_eq!(
            extract_session_id(&notification).as_deref(),
            Some("thread-123")
        );
        assert_eq!(turn_id(&notification), "turn-123");
    }

    #[test]
    fn installs_and_restores_existing_desktop_notification() {
        let directory = env::temp_dir().join(format!("tally-notify-config-{}", unique_suffix()));
        let hooks_path = directory.join("hooks.json");
        let config_path = codex_config_path_for_hooks_path(&hooks_path);
        let state_dir = state_dir_for_hooks_path(&hooks_path);
        let binary_path = installed_binary_path_for_hooks_path(&hooks_path);
        let saved_path = previous_notify_path(&state_dir);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            &config_path,
            "# keep this comment\nmodel = \"gpt-test\"\nnotify = [\"keep-notify\", \"--flag\"]\n",
        )
        .unwrap();

        install_desktop_notify(&config_path, &binary_path, &state_dir, &saved_path).unwrap();
        let installed = read_codex_config(&config_path).unwrap();
        assert_eq!(installed["model"].as_str(), Some("gpt-test"));
        assert_eq!(
            toml_notify_command(&installed).unwrap(),
            Some(tally_notify_command(&binary_path, &state_dir))
        );
        assert_eq!(
            read_json_file(&saved_path).unwrap()["command"],
            json!(["keep-notify", "--flag"])
        );

        install_desktop_notify(&config_path, &binary_path, &state_dir, &saved_path).unwrap();
        assert_eq!(
            read_json_file(&saved_path).unwrap()["command"],
            json!(["keep-notify", "--flag"])
        );
        uninstall_desktop_notify(&config_path, &binary_path, &state_dir, &saved_path).unwrap();
        let restored = read_codex_config(&config_path).unwrap();
        assert_eq!(
            toml_notify_command(&restored).unwrap(),
            Some(vec!["keep-notify".to_string(), "--flag".to_string()])
        );
        assert!(fs::read_to_string(&config_path)
            .unwrap()
            .contains("# keep this comment"));
        assert!(!saved_path.exists());
        fs::remove_dir_all(directory).unwrap();
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
        assert_eq!(record_type_for_hook("Other"), "CODEX_LIFECYCLE");
    }

    #[test]
    fn only_session_end_stops_the_heartbeat_daemon() {
        assert!(!heartbeat_stop_requested("Stop"));
        assert!(heartbeat_stop_requested("SessionEnd"));
    }

    #[test]
    fn installs_turn_and_session_end_hooks() {
        assert!(EVENTS.iter().any(|event| event.name == "Stop"));
        assert!(EVENTS.iter().any(|event| event.name == "SessionEnd"));
    }

    #[test]
    fn emits_heartbeat_only_after_a_quiet_interval() {
        assert!(!heartbeat_due(0, 600));
        assert!(!heartbeat_due(599, 600));
        assert!(heartbeat_due(600, 600));
        assert!(heartbeat_due(1_200, 600));
    }

    #[test]
    fn allows_only_one_heartbeat_daemon_lock() {
        let directory = env::temp_dir().join(format!("tally-heartbeat-{}", unique_suffix()));
        let path = directory.join("session.pid");
        let first = claim_heartbeat_daemon(&path).unwrap().unwrap();
        assert!(claim_heartbeat_daemon(&path).unwrap().is_none());
        FileExt::unlock(&first).unwrap();
        drop(first);
        let second = claim_heartbeat_daemon(&path).unwrap().unwrap();
        FileExt::unlock(&second).unwrap();
        drop(second);
        fs::remove_dir_all(directory).unwrap();
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
