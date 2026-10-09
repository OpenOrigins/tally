use fs2::FileExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use tally_common::agent_runtime::{
    backup_if_exists, env_enabled, expand_home, first_string_by_key, home_dir, hook_command,
    light_git_state, parse_payload, random_hex, read_json_file, read_stdin, remove_file_if_exists,
    run_id, safe_slug, set_default, sha256_str, stable_id, unique_suffix, utc_now, workspace_path,
    write_json_atomic, write_text_atomic, AuditSink, AuditSinkConfig, HeartbeatFiles,
};
use toml_edit::{
    value as toml_value, Array as TomlArray, ArrayOfTables, DocumentMut, Item as TomlItem,
    Table as TomlTable, Value as TomlValue,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const EVENTS: &[HookEvent] = &[
    HookEvent::new("SessionStart", Some("*")),
    HookEvent::new("UserPromptSubmit", None),
    HookEvent::new("PreToolUse", Some("*")),
    HookEvent::new("PermissionRequest", Some("*")),
    HookEvent::new("PostToolUse", Some("*")),
    HookEvent::new("PreCompact", Some("*")),
    HookEvent::new("PostCompact", Some("*")),
    HookEvent::new("SubagentStart", Some("*")),
    HookEvent::new("SubagentStop", Some("*")),
    HookEvent::new("Stop", None),
    HookEvent::new("SessionEnd", None),
];

const CODEX_CLI_INSTALL_URL: &str = "https://developers.openai.com/codex/cli";

#[derive(Clone, Debug)]
pub struct CodexCliStatus {
    pub command: PathBuf,
    pub version: String,
}

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
        "tally-codex {}\n\nCommands:\n  gui           Open the graphical installer\n  install --api-key <KEY> [--api-url <URL>] [--config-path <PATH>]\n                Install Codex hooks, then run `codex` to review and trust them\n  uninstall [--config-path <PATH>]\n                Remove Tally hooks and local credentials\n  wrap [ARGS]   Run Codex through Tally\n  hook EVENT    Record a hook event\n  notify        Record a Codex Desktop turn notification\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn record_hook_event(event_type: &str) -> Result<()> {
    let raw = read_stdin()?;
    let payload = parse_payload(&raw);
    if tally_common::privacy::capture_blocked(&workspace_path(), &payload) {
        return Ok(());
    }
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
    if tally_common::privacy::capture_blocked(&workspace_path(), payload) {
        return Ok(());
    }
    set_runtime_defaults();
    if env::var("TALLY_RUN_ID").unwrap_or_default().is_empty() {
        if let Some(run_id) = derive_run_id(payload) {
            env::set_var("TALLY_RUN_ID", run_id);
        }
    }

    let sink = audit_sink(source)?;
    let raw_ref = sink.private_payload(payload)?;
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
    let event_id = format!("evt_{}", random_hex(16));
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

    let mut record = build_tally_record(&sink, event_type, payload, &raw_ref, &metadata)?;
    record["record_id"] = Value::String(format!(
        "rec_{}",
        event["event_id"]
            .as_str()
            .unwrap_or("evt_unknown")
            .trim_start_matches("evt_")
    ));
    record["audit_event_id"] = event["event_id"].clone();
    record["token_usage"] = extract_session_id(payload)
        .map(|session_id| codex_token_usage(&codex_home_dir(), &session_id))
        .unwrap_or_else(|| json!({"available": false}));
    sink.write_tally_record(&record)?;
    Ok(())
}

/// Returns the session's token usage across its Codex CLI rollout files, in
/// the shape of the OpenAI Responses API `usage` object.
///
/// When usage is unavailable, `reason` says why: `rollout_not_found` means
/// Codex never persisted the thread (ephemeral threads such as
/// `codex exec --ephemeral` and Codex Desktop's title/suggestion helpers),
/// `token_count_not_found` means the rollout exists but has no usage yet.
fn codex_token_usage(codex_home: &Path, session_id: &str) -> Value {
    let rollout_paths = find_codex_rollout_files(codex_home, session_id);
    if rollout_paths.is_empty() {
        return json!({"available": false, "reason": "rollout_not_found"});
    }

    let Some(info) = rollout_paths.iter().find_map(|path| {
        let mut file = fs::File::open(path).ok()?;
        latest_codex_token_info(&mut file)
    }) else {
        return json!({"available": false, "reason": "token_count_not_found"});
    };
    let usage = &info["total_token_usage"];
    // A fork's counter starts at its parent's total; report only the fork's own usage.
    let baseline = rollout_paths
        .last()
        .and_then(|path| codex_rollout_baseline(path, true, false))
        .unwrap_or(Value::Null);
    let keys = [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cached_input_tokens",
        "reasoning_output_tokens",
    ];
    let newest: [u64; 5] = keys.map(|key| {
        usage
            .get(key)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_sub(baseline.get(key).and_then(Value::as_u64).unwrap_or(0))
    });

    // A continuation may carry the previous counter, reset it, or branch from
    // an earlier point. Count the usage added in each file once.
    let mut stitched = [0_u64; 5];
    for (index, path) in rollout_paths.iter().rev().enumerate() {
        let Some(mut file) = fs::File::open(path).ok() else {
            continue;
        };
        let Some(info) = latest_codex_token_info(&mut file) else {
            continue;
        };
        let total = &info["total_token_usage"];
        let file_baseline = if index == 0 {
            baseline.clone()
        } else {
            let Some(value) = codex_rollout_baseline(path, false, true) else {
                continue;
            };
            value
        };
        for (field, key) in keys.iter().enumerate() {
            let own = total
                .get(key)
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .saturating_sub(file_baseline.get(key).and_then(Value::as_u64).unwrap_or(0));
            stitched[field] = stitched[field].saturating_add(own);
        }
    }
    // Keep the newest cumulative total if an earlier rollout is unavailable.
    let fields = if stitched[2] > newest[2] {
        stitched
    } else {
        newest
    };
    let mut result = json!({
        "available": true,
        "input_tokens": fields[0],
        "output_tokens": fields[1],
        "total_tokens": fields[2],
        "input_token_details": {
            "cached_tokens": fields[3],
        },
        "output_token_details": {
            "reasoning_tokens": fields[4],
        },
    });
    // The latest turn's usage is what is in the context window right now.
    if let Some(context_tokens) = info["last_token_usage"]["total_tokens"].as_u64() {
        result["context_tokens"] = json!(context_tokens);
    }
    if let Some(context_window) = info["model_context_window"].as_u64() {
        result["context_window"] = json!(context_window);
    }
    result
}

fn latest_codex_token_info(file: &mut fs::File) -> Option<Value> {
    const CHUNK_SIZE: usize = 64 * 1024;

    let mut position = file.seek(SeekFrom::End(0)).ok()?;
    let mut partial_line = Vec::new();
    while position > 0 {
        let read_len = usize::try_from(position.min(CHUNK_SIZE as u64)).ok()?;
        position -= read_len as u64;
        file.seek(SeekFrom::Start(position)).ok()?;

        let mut chunk = vec![0; read_len];
        file.read_exact(&mut chunk).ok()?;
        chunk.extend_from_slice(&partial_line);

        let mut line_end = chunk.len();
        while let Some(newline) = chunk[..line_end].iter().rposition(|byte| *byte == b'\n') {
            if let Some(info) = codex_token_info_from_line(&chunk[newline + 1..line_end]) {
                return Some(info);
            }
            line_end = newline;
        }
        partial_line = chunk[..line_end].to_vec();
    }

    codex_token_info_from_line(&partial_line)
}

/// Counter value before a rollout's first usage event. The original file needs
/// this adjustment only for forks; every continuation needs it to avoid
/// recounting usage inherited from an earlier file.
fn codex_rollout_baseline(path: &Path, require_fork: bool, require_last: bool) -> Option<Value> {
    let mut lines = BufReader::new(fs::File::open(path).ok()?).lines();
    let meta = serde_json::from_str::<Value>(&lines.next()?.ok()?).ok()?;
    if meta.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    if require_fork {
        meta.get("payload")?.get("forked_from_id")?.as_str()?;
    }

    let info = lines.map_while(|line| line.ok()).find_map(|line| {
        let entry = serde_json::from_str::<Value>(&line).ok()?;
        let payload = entry.get("payload")?;
        if entry.get("type").and_then(Value::as_str) != Some("event_msg")
            || payload.get("type").and_then(Value::as_str) != Some("token_count")
        {
            return None;
        }
        let info = payload.get("info")?;
        info.get("total_token_usage")?.as_object()?;
        Some(info.clone())
    })?;
    let total = info.get("total_token_usage")?.as_object()?;
    let last = info.get("last_token_usage").and_then(Value::as_object);
    if require_last && last.is_none() {
        return None;
    }
    let inherited = total
        .iter()
        .filter_map(|(key, value)| {
            let own = last
                .and_then(|l| l.get(key))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Some((key.clone(), json!(value.as_u64()?.saturating_sub(own))))
        })
        .collect::<serde_json::Map<_, _>>();
    Some(Value::Object(inherited))
}

/// The `info` of a `token_count` event that carries a cumulative total.
fn codex_token_info_from_line(line: &[u8]) -> Option<Value> {
    let entry = serde_json::from_slice::<Value>(line).ok()?;
    if entry.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = entry.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let info = payload.get("info")?;
    info.get("total_token_usage")?;
    Some(info.clone())
}

/// Returns every rollout file for the session, newest first. When Codex
/// Desktop resumes a closed thread it starts a continuation file named
/// `rollout-<timestamp>-<thread-id>_<new-id>.jsonl`; file names begin with
/// their creation timestamp, so they sort chronologically.
fn find_codex_rollout_files(codex_home: &Path, session_id: &str) -> Vec<PathBuf> {
    let session_marker = format!("-{session_id}");
    let mut matches = Vec::new();
    let mut directories = vec![codex_home.join("sessions")];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".jsonl"))
                    .and_then(|stem| stem.rsplit_once(&session_marker))
                    .is_some_and(|(_, trailing)| trailing.is_empty() || trailing.starts_with('_'))
            {
                matches.push(path);
            }
        }
    }
    matches.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    matches
}

fn codex_home_dir() -> PathBuf {
    expand_home(&env::var("CODEX_HOME").unwrap_or_else(|_| format!("{}/.codex", home_dir())))
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
    if tally_common::privacy::capture_blocked(&workspace_path(), &payload) {
        return Ok(());
    }
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

    let messages = payload["input-messages"].as_array();
    let prompt = messages
        .filter(|messages| !messages.is_empty() && messages.iter().all(Value::is_string))
        .map(|messages| {
            messages
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        });
    let mut instruction = base.clone();
    instruction["prompt"] = prompt.map(Value::String).unwrap_or(Value::Null);
    instruction["input_messages"] = payload["input-messages"].clone();
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

    if args.first().map(String::as_str) == Some("exec")
        && env_enabled("TALLY_TEE_CODEX_STDIO", false)
        && !tally_common::privacy::capture_blocked(&workspace_path(), &Value::Null)
    {
        run_codex_with_tee(&args)
    } else {
        let status = Command::new("codex").args(&args).status()?;
        Ok(status.code().unwrap_or(1))
    }
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
    tally_common::agent_runtime::update_heartbeat_state(
        sink,
        &HeartbeatFiles::new(&log_root(), &sink.run_id),
        "codex",
        event_type,
        extract_session_id(payload),
        observed_at,
    )
}

fn run_heartbeat_daemon() -> Result<()> {
    set_runtime_defaults();
    if tally_common::privacy::capture_blocked(&workspace_path(), &Value::Null) {
        return Ok(());
    }
    let sink = audit_sink("hook-heartbeat")?;
    tally_common::agent_runtime::run_heartbeat_daemon(
        &sink,
        &HeartbeatFiles::new(&log_root(), &sink.run_id),
    )
}

pub fn codex_cli_status() -> Result<CodexCliStatus> {
    let configured = env::var_os("TALLY_CODEX_CLI").map(PathBuf::from);
    let candidates = configured
        .clone()
        .map(|path| vec![path])
        .unwrap_or_else(codex_cli_candidates);
    let mut last_error = None;

    for candidate in candidates {
        let version = match run_codex_cli(&candidate, &["--version"]) {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            }
            Ok(output) => {
                last_error = Some(format!(
                    "{} exited with status {}",
                    candidate.display(),
                    output.status
                ));
                continue;
            }
            Err(error) => {
                last_error = Some(format!("{}: {error}", candidate.display()));
                continue;
            }
        };
        let features = match run_codex_cli_features(&candidate) {
            Ok(output) => output,
            Err(error) => {
                last_error = Some(format!(
                    "{} could not inspect hook support: {error}",
                    candidate.display()
                ));
                continue;
            }
        };
        if !features.status.success() {
            last_error = Some(format!("Codex CLI {version} could not verify hook support"));
            continue;
        }
        let hooks_enabled = String::from_utf8_lossy(&features.stdout)
            .lines()
            .any(|line| {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                fields.first() == Some(&"hooks") && fields.last() == Some(&"true")
            });
        if !hooks_enabled {
            last_error = Some(format!(
                "Codex CLI {version} does not have lifecycle hooks enabled"
            ));
            continue;
        }
        return Ok(CodexCliStatus {
            command: candidate,
            version,
        });
    }

    let detail = last_error
        .map(|error| format!(" Last check: {error}."))
        .unwrap_or_default();
    Err(format!(
        "Codex CLI with lifecycle hook support is required. Install it from {CODEX_CLI_INSTALL_URL}, confirm `codex --version` works, and reopen Tally.{detail}"
    )
    .into())
}

pub fn codex_hook_approval_instructions() -> String {
    "Open a terminal and run `codex`. When Codex says hooks need review, choose `Review hooks`, inspect the OpenOrigins Tally commands, then press `t` to trust all. Quit the CLI and reopen Codex Desktop afterward."
        .to_string()
}

fn codex_cli_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            #[cfg(windows)]
            for name in ["codex.exe", "codex.cmd", "codex.bat"] {
                push_existing_candidate(&mut candidates, directory.join(name));
            }
            #[cfg(not(windows))]
            push_existing_candidate(&mut candidates, directory.join("codex"));
        }
    }

    #[cfg(target_os = "macos")]
    for path in ["/opt/homebrew/bin/codex", "/usr/local/bin/codex"] {
        push_existing_candidate(&mut candidates, PathBuf::from(path));
    }

    #[cfg(not(windows))]
    let home = PathBuf::from(home_dir());
    #[cfg(not(windows))]
    for path in [
        home.join(".local/bin/codex"),
        home.join(".bun/bin/codex"),
        home.join(".npm-global/bin/codex"),
    ] {
        push_existing_candidate(&mut candidates, path);
    }
    #[cfg(windows)]
    if let Ok(app_data) = env::var("APPDATA") {
        for name in ["codex.cmd", "codex.exe"] {
            push_existing_candidate(
                &mut candidates,
                PathBuf::from(&app_data).join("npm").join(name),
            );
        }
    }
    candidates
}

fn push_existing_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_file() && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn run_codex_cli(path: &Path, arguments: &[&str]) -> io::Result<std::process::Output> {
    #[cfg(windows)]
    if matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("cmd" | "bat")
    ) {
        return Command::new("cmd")
            .arg("/C")
            .arg(path)
            .args(arguments)
            .output();
    }
    Command::new(path).args(arguments).output()
}

fn run_codex_cli_features(path: &Path) -> io::Result<std::process::Output> {
    let configured_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home_dir()).join(".codex"));
    if configured_home.is_dir() {
        return run_codex_cli(path, &["features", "list"]);
    }

    let temporary_home = env::temp_dir().join(format!(
        "tally-codex-preflight-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&temporary_home)?;
    let output = run_codex_cli_with_home(path, &["features", "list"], &temporary_home);
    let cleanup_result = fs::remove_dir_all(&temporary_home);
    match (output, cleanup_result) {
        (Ok(output), _) => Ok(output),
        (Err(error), _) => Err(error),
    }
}

fn run_codex_cli_with_home(
    path: &Path,
    arguments: &[&str],
    codex_home: &Path,
) -> io::Result<std::process::Output> {
    #[cfg(windows)]
    if matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("cmd" | "bat")
    ) {
        return Command::new("cmd")
            .arg("/C")
            .arg(path)
            .args(arguments)
            .env("CODEX_HOME", codex_home)
            .output();
    }
    Command::new(path)
        .args(arguments)
        .env("CODEX_HOME", codex_home)
        .output()
}

pub fn install_desktop_hooks(
    options: tally_common::InstallOptions,
) -> Result<tally_common::InstallReport> {
    let codex_cli = codex_cli_status()?;
    let config_path = effective_config_path(options.config_path.as_deref());
    let state_dir = state_dir_for_config_path(&config_path);
    let installed_binary_path = installed_binary_path_for_config_path(&config_path);
    let previous_notify_path = previous_notify_path(&state_dir);
    fs::create_dir_all(config_path.parent().unwrap_or_else(|| Path::new(".")))?;
    tally_common::mark_tally_data_directory(&log_root())?;
    let source_binary = tally_common::installation_source_executable()?;
    let hook_bin = installed_binary_path.display().to_string();

    let config_existed = config_path.exists();
    let mut document = read_codex_config(&config_path)?;
    remove_tally_config_hooks(&mut document)?;
    install_tally_config_hooks(&mut document, &hook_bin, &state_dir)?;

    let backup = backup_if_exists(&config_path)?;
    let config_snapshot = tally_common::FileSnapshot::capture(&config_path)?;
    let previous_notify_snapshot = tally_common::FileSnapshot::capture(&previous_notify_path)?;
    let key_snapshot =
        tally_common::FileSnapshot::capture(&tally_common::api_key_path(&state_dir))?;
    let api_config_snapshot =
        tally_common::FileSnapshot::capture(&tally_common::config_path(&state_dir))?;
    let agent_id_snapshot = tally_common::FileSnapshot::capture(&state_dir.join("agent-id.txt"))?;
    let binary_snapshot = tally_common::FileSnapshot::capture(&installed_binary_path)?;

    let install_result = (|| -> Result<()> {
        tally_common::install_executable(&source_binary, &installed_binary_path)?;
        tally_common::load_or_create_agent_id(&state_dir, "codex")?;
        tally_common::write_credentials(&state_dir, &options)?;
        write_text_atomic(&config_path, &document.to_string())?;
        install_desktop_notify(
            &config_path,
            &installed_binary_path,
            &state_dir,
            &previous_notify_path,
            config_existed,
        )?;
        Ok(())
    })();
    if let Err(error) = install_result {
        return Err(tally_common::install_error_with_rollback(
            error,
            &[
                &config_snapshot,
                &previous_notify_snapshot,
                &key_snapshot,
                &api_config_snapshot,
                &agent_id_snapshot,
                &binary_snapshot,
            ],
        ));
    }
    println!("Installed Tally Codex hooks into {}", config_path.display());
    if let Some(backup) = backup.as_ref() {
        println!("Backed up previous Codex config to {}", backup.display());
    }
    println!(
        "Codex CLI: {} ({})",
        codex_cli.version,
        codex_cli.command.display()
    );
    println!("Hook binary: {hook_bin}");
    println!("Codex Desktop notifications: {}", config_path.display());
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
    let approval_instructions = codex_hook_approval_instructions();
    println!("Action required: {approval_instructions}");
    Ok(tally_common::InstallReport {
        config_path,
        state_dir,
        logs_path: log_root(),
        installed_binary_path,
        backup_path: backup,
        handshake_error,
        approval_required: true,
        approval_instructions: Some(approval_instructions),
        client_version: Some(codex_cli.version),
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
    uninstall_desktop_notify(
        &config_path,
        &installed_binary_path_for_config_path(&config_path),
        &state_dir,
        &previous_notify_path(&state_dir),
    )?;
    if config_path.exists() {
        let backup = backup_if_exists(&config_path)?;
        let mut document = read_codex_config(&config_path)?;
        let removed = remove_tally_config_hooks(&mut document)?;
        if document.is_empty() {
            remove_file_if_exists(&config_path)?;
        } else {
            write_text_atomic(&config_path, &document.to_string())?;
        }
        println!(
            "Removed {removed} Tally Codex hook handler(s) from {}",
            config_path.display()
        );
        if let Some(backup) = backup {
            println!("Backed up previous Codex config to {}", backup.display());
        }
    } else {
        println!("No Codex config found at {}", config_path.display());
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
        agent_name: "Codex",
        hook_field: "codex_hook_event",
        lifecycle_record_type: "CODEX_LIFECYCLE",
        default_tool_server: "codex",
        prompt_summary_label: "User prompt submitted to Codex",
        result_summary_label: "Codex reported a tool result",
        tool_param_keys: &["arguments", "args", "params", "input"],
        instruction_id_keys: &["instruction_id", "turn_id", "turnId"],
        tool_server_keys: &["server", "server_name", "mcp_server", "recipient_namespace"],
        tool_name_keys: &[
            "tool_name",
            "toolName",
            "name",
            "command",
            "mcp_tool_name",
            "recipient_name",
        ],
        error_keys: &["error", "exception"],
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
        "SessionStart" => "SESSION_START",
        "UserPromptSubmit" => "INSTRUCTION_RECEIVED",
        "PreToolUse" | "PermissionRequest" => "ACTION_TAKEN",
        "PostToolUse" => "RESULT_RECEIVED",
        "Stop" => "TURN_END",
        "SessionEnd" => "SESSION_END",
        "SubagentStart" | "SubagentStop" => "HANDOFF",
        _ => "CODEX_LIFECYCLE",
    }
}

#[derive(Clone, Copy)]
struct HookEvent {
    name: &'static str,
    matcher: Option<&'static str>,
}

impl HookEvent {
    const fn new(name: &'static str, matcher: Option<&'static str>) -> Self {
        Self { name, matcher }
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
        heartbeat_client: "codex",
        event_schema: "tally-codex.v1",
    })
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

fn install_tally_config_hooks(
    document: &mut DocumentMut,
    hook_bin: &str,
    state_dir: &Path,
) -> Result<()> {
    if !document.contains_key("hooks") {
        document["hooks"] = TomlItem::Table(TomlTable::new());
    }
    let hooks = document["hooks"]
        .as_table_mut()
        .ok_or("Codex config `hooks` must be a table")?;

    for event in EVENTS {
        if !hooks.contains_key(event.name) {
            hooks[event.name] = TomlItem::ArrayOfTables(ArrayOfTables::new());
        }
        let groups = hooks[event.name].as_array_of_tables_mut().ok_or_else(|| {
            format!(
                "Codex config `hooks.{}` must be an array of tables",
                event.name
            )
        })?;
        let mut group = TomlTable::new();
        if event.matcher.is_some() {
            group["matcher"] = toml_value(".*");
        }
        let mut handlers = ArrayOfTables::new();
        let mut handler = TomlTable::new();
        handler["type"] = toml_value("command");
        handler["command"] = toml_value(hook_command(hook_bin, "codex", event.name, state_dir));
        handler["timeout"] = toml_value(if event.name == "SessionEnd" { 3 } else { 15 });
        handlers.push(handler);
        group["hooks"] = TomlItem::ArrayOfTables(handlers);
        groups.push(group);
    }
    Ok(())
}

fn remove_tally_config_hooks(document: &mut DocumentMut) -> Result<usize> {
    let Some(item) = document.get_mut("hooks") else {
        return Ok(0);
    };
    let hooks = item
        .as_table_mut()
        .ok_or("Codex config `hooks` must be a table")?;
    let events = hooks
        .iter()
        .filter(|(name, _)| *name != "state")
        .map(|(name, _)| name.to_string())
        .collect::<Vec<_>>();
    let mut removed = 0;
    let mut empty_events = Vec::new();

    for event in events {
        let Some(groups) = hooks
            .get_mut(&event)
            .and_then(TomlItem::as_array_of_tables_mut)
        else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(handlers) = group
                .get_mut("hooks")
                .and_then(TomlItem::as_array_of_tables_mut)
            else {
                continue;
            };
            let before = handlers.len();
            handlers.retain(|handler| {
                let command = handler
                    .get("command")
                    .and_then(TomlItem::as_str)
                    .unwrap_or("");
                !is_tally_hook_command(command)
            });
            removed += before - handlers.len();
        }
        groups.retain(|group| {
            group
                .get("hooks")
                .and_then(TomlItem::as_array_of_tables)
                .is_none_or(|handlers| !handlers.is_empty())
        });
        if groups.is_empty() {
            empty_events.push(event);
        }
    }
    for event in empty_events {
        hooks.remove(&event);
    }
    if hooks.is_empty() {
        document.remove("hooks");
    }
    Ok(removed)
}

fn is_tally_hook_command(command: &str) -> bool {
    let current = command.contains("tally-codex") && command.contains(" hook ");
    let unified = command.contains("tally") && command.contains(" codex hook ");
    current || unified
}

fn install_desktop_notify(
    config_path: &Path,
    binary_path: &Path,
    state_dir: &Path,
    saved_notify_path: &Path,
    config_existed: bool,
) -> Result<()> {
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

fn default_log_root() -> String {
    format!("{}/.tally-codex/logs", home_dir())
}

fn log_root() -> PathBuf {
    expand_home(&env::var("TALLY_LOG_ROOT").unwrap_or_else(|_| default_log_root()))
}

pub fn default_config_path() -> PathBuf {
    if let Ok(path) = env::var("CODEX_CONFIG_PATH") {
        return expand_home(&path);
    }
    if let Ok(path) = env::var("CODEX_HOOKS_PATH") {
        return expand_home(&path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("config.toml");
    }
    let codex_home = env::var("CODEX_HOME").unwrap_or_else(|_| format!("{}/.codex", home_dir()));
    expand_home(&format!("{codex_home}/config.toml"))
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
        previous_notify_path(&state_dir),
        tally_common::api_key_path(&state_dir),
        tally_common::config_path(&state_dir),
        state_dir.join("agent-id.txt"),
        installed_binary_path_for_config_path(&config_path),
    ]
}

fn installed_binary_path_for_config_path(path: &Path) -> PathBuf {
    tally_common::installed_executable_path(path, "tally-codex")
}

fn remove_local_credentials_for_config_path(path: &Path) -> Result<()> {
    let state_dir = state_dir_for_config_path(path);
    for path in [
        tally_common::api_key_path(&state_dir),
        tally_common::config_path(&state_dir),
        previous_notify_path(&state_dir),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    tally_common::remove_installed_executable(&installed_binary_path_for_config_path(path))
}

fn agent_id() -> Result<String> {
    tally_common::load_or_create_agent_id(&onboarding_state_dir(), "codex")
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
    fn reads_latest_cumulative_codex_token_usage() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a0b49b-d186-7172-9301-1d8037168b23";
        let sessions_dir = codex_home.join("sessions/2026/09/18");
        fs::create_dir_all(&sessions_dir).unwrap();
        let rollout_path =
            sessions_dir.join(format!("rollout-2026-09-18T18-31-45-{session_id}.jsonl"));
        let mut file = fs::File::create(&rollout_path).unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 14465,
                            "cached_input_tokens": 9984,
                            "cache_write_input_tokens": 0,
                            "output_tokens": 9,
                            "reasoning_output_tokens": 0,
                            "total_tokens": 14474,
                        }
                    }
                }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 43454,
                            "cached_input_tokens": 38144,
                            "cache_write_input_tokens": 0,
                            "output_tokens": 113,
                            "reasoning_output_tokens": 12,
                            "total_tokens": 43567,
                        }
                    }
                }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": "x".repeat(70 * 1024),
                }
            })
        )
        .unwrap();

        let usage = codex_token_usage(&codex_home, session_id);
        assert_eq!(
            usage,
            json!({
                "available": true,
                "input_tokens": 43454,
                "output_tokens": 113,
                "total_tokens": 43567,
                "input_token_details": {"cached_tokens": 38144},
                "output_token_details": {"reasoning_tokens": 12},
            })
        );

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn reads_codex_usage_from_collision_suffixed_rollout() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a0ae4a-48fb-7570-8c86-b59934a2cdfe";
        let sessions_dir = codex_home.join("sessions/2026/09/17");
        fs::create_dir_all(&sessions_dir).unwrap();
        let rollout_path = sessions_dir.join(format!(
            "rollout-2026-09-17T23-28-00-{session_id}_01a0b10e-0282-7190-b9a8-456dba158c25.jsonl"
        ));
        let mut file = fs::File::create(&rollout_path).unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 100,
                            "cached_input_tokens": 75,
                            "output_tokens": 25,
                            "reasoning_output_tokens": 5,
                            "total_tokens": 125,
                        }
                    }
                }
            })
        )
        .unwrap();

        let usage = codex_token_usage(&codex_home, session_id);
        assert_eq!(usage["available"], true);
        assert_eq!(usage["total_tokens"], 125);

        fs::remove_dir_all(codex_home).unwrap();
    }

    fn write_codex_token_count(path: &Path, total_tokens: u64) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {"total_token_usage": {"total_tokens": total_tokens}}
                }
            })
        )
        .unwrap();
    }

    #[test]
    fn reads_codex_usage_from_newest_continuation_rollout() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a10fa1-901e-7141-bae3-6d61bcf8632a";
        let first_day = codex_home.join("sessions/2026/10/06");
        let next_day = codex_home.join("sessions/2026/10/07");
        fs::create_dir_all(&first_day).unwrap();
        fs::create_dir_all(&next_day).unwrap();
        let original = first_day.join(format!("rollout-2026-10-06T10-43-28-{session_id}.jsonl"));
        let continuation = first_day.join(format!(
            "rollout-2026-10-06T10-44-41-{session_id}_01a10fa2-abb2-74f3-b82b-e5bcef3cc9ed.jsonl"
        ));
        let second_continuation = next_day.join(format!(
            "rollout-2026-10-07T09-00-00-{session_id}_01a10fa3-0000-7000-8000-000000000000.jsonl"
        ));
        write_codex_token_count(&original, 133_243);
        write_codex_token_count(&continuation, 248_538);
        write_codex_token_count(&second_continuation, 300_000);

        assert_eq!(
            codex_token_usage(&codex_home, session_id)["total_tokens"],
            300_000
        );

        fs::write(&second_continuation, "").unwrap();
        assert_eq!(
            codex_token_usage(&codex_home, session_id)["total_tokens"],
            248_538
        );

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn reads_codex_context_from_the_newest_token_count() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a10fa1-901e-7141-bae3-6d61bcf8632b";
        let sessions_dir = codex_home.join("sessions/2026/10/08");
        fs::create_dir_all(&sessions_dir).unwrap();
        let rollout_path =
            sessions_dir.join(format!("rollout-2026-10-08T10-00-00-{session_id}.jsonl"));
        let mut file = fs::File::create(&rollout_path).unwrap();
        for (total, last) in [(50_000, 50_000), (180_000, 130_000)] {
            writeln!(
                file,
                "{}",
                json!({
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {"total_tokens": total},
                            "last_token_usage": {"input_tokens": last - 1_000, "output_tokens": 1_000, "total_tokens": last},
                            "model_context_window": 272_000,
                        }
                    }
                })
            )
            .unwrap();
        }

        let usage = codex_token_usage(&codex_home, session_id);
        assert_eq!(usage["total_tokens"], 180_000);
        assert_eq!(usage["context_tokens"], 130_000);
        assert_eq!(usage["context_window"], 272_000);

        fs::remove_dir_all(codex_home).unwrap();
    }

    fn write_codex_session_meta(path: &Path, session_id: &str, forked_from: Option<&str>) {
        let mut payload = json!({"id": session_id, "session_id": session_id});
        if let Some(parent) = forked_from {
            payload["forked_from_id"] = json!(parent);
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(
            file,
            "{}",
            json!({"type": "session_meta", "payload": payload})
        )
        .unwrap();
    }

    fn write_codex_token_count_with_last(path: &Path, total: u64, last: u64) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": total - 10, "output_tokens": 10, "total_tokens": total},
                        "last_token_usage": {"input_tokens": last - 10, "output_tokens": 10, "total_tokens": last}
                    }
                }
            })
        )
        .unwrap();
    }

    #[test]
    fn continuation_that_resets_its_counter_keeps_original_usage() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a03e98-521d-75a2-a5d2-047cb4229fee";
        let sessions_dir = codex_home.join("sessions/2026/08/26");
        fs::create_dir_all(&sessions_dir).unwrap();
        let original = sessions_dir.join(format!("rollout-2026-08-26T18-02-45-{session_id}.jsonl"));
        let continuation = sessions_dir.join(format!(
            "rollout-2026-08-26T18-33-42-{session_id}_01a03eb4-aac1-7f11-bb6d-39a19e495063.jsonl"
        ));
        write_codex_session_meta(&original, session_id, None);
        write_codex_token_count_with_last(&original, 16_999, 16_999);
        write_codex_token_count_with_last(&original, 7_524_110, 166_994);
        write_codex_session_meta(&continuation, session_id, None);
        write_codex_token_count_with_last(&continuation, 167_521, 167_521);
        write_codex_token_count_with_last(&continuation, 337_825, 170_304);

        assert_eq!(
            codex_token_usage(&codex_home, session_id)["total_tokens"],
            7_861_935
        );

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn continuations_from_the_same_snapshot_each_contribute_usage() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a0ae4a-48fb-7570-8c86-b59934a2cdfe";
        let sessions_dir = codex_home.join("sessions/2026/09/17");
        fs::create_dir_all(&sessions_dir).unwrap();
        let original = sessions_dir.join(format!("rollout-2026-09-17T10-34-59-{session_id}.jsonl"));
        let first_continuation = sessions_dir.join(format!(
            "rollout-2026-09-17T19-21-04-{session_id}_01a0b02b-ee7f-7e10-bd53-bd2dada435cf.jsonl"
        ));
        let second_continuation = sessions_dir.join(format!(
            "rollout-2026-09-17T19-42-53-{session_id}_01a0b03f-e7d6-78d3-b30a-0f05f4f26ccd.jsonl"
        ));
        write_codex_session_meta(&original, session_id, None);
        write_codex_token_count_with_last(&original, 80_000, 80_000);
        write_codex_token_count_with_last(&original, 100_000, 20_000);
        write_codex_session_meta(&first_continuation, session_id, None);
        write_codex_token_count_with_last(&first_continuation, 85_000, 5_000);
        write_codex_token_count_with_last(&first_continuation, 90_000, 5_000);
        write_codex_session_meta(&second_continuation, session_id, None);
        write_codex_token_count_with_last(&second_continuation, 87_000, 7_000);
        write_codex_token_count_with_last(&second_continuation, 110_000, 23_000);

        assert_eq!(
            codex_token_usage(&codex_home, session_id)["total_tokens"],
            140_000
        );

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn forked_codex_rollout_reports_only_its_own_usage() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a10ffe-fa34-7f33-9f68-56dad306d455";
        let sessions_dir = codex_home.join("sessions/2026/10/06");
        fs::create_dir_all(&sessions_dir).unwrap();
        let fork = sessions_dir.join(format!("rollout-2026-10-06T12-25-30-{session_id}.jsonl"));
        write_codex_session_meta(
            &fork,
            session_id,
            Some("01a10ffd-846f-7191-beb4-4205e87a9d4b"),
        );
        // Parent had used 88,500 tokens when it was forked.
        write_codex_token_count_with_last(&fork, 103_916, 15_416);
        write_codex_token_count_with_last(&fork, 119_366, 15_450);

        let usage = codex_token_usage(&codex_home, session_id);
        assert_eq!(usage["total_tokens"], 30_866);
        assert_eq!(usage["output_tokens"], 10);
        assert_eq!(usage["input_tokens"], 30_856);

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn resumed_fork_subtracts_inherited_usage_from_continuation() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a10ffe-fa34-7f33-9f68-56dad306d455";
        let sessions_dir = codex_home.join("sessions/2026/10/06");
        fs::create_dir_all(&sessions_dir).unwrap();
        let fork = sessions_dir.join(format!("rollout-2026-10-06T12-25-30-{session_id}.jsonl"));
        let continuation = sessions_dir.join(format!(
            "rollout-2026-10-06T13-00-00-{session_id}_01a11000-0000-7000-8000-000000000000.jsonl"
        ));
        write_codex_session_meta(
            &fork,
            session_id,
            Some("01a10ffd-846f-7191-beb4-4205e87a9d4b"),
        );
        write_codex_token_count_with_last(&fork, 103_916, 15_416);
        write_codex_session_meta(&continuation, session_id, None);
        write_codex_token_count_with_last(&continuation, 150_000, 20_000);

        assert_eq!(
            codex_token_usage(&codex_home, session_id)["total_tokens"],
            61_500
        );

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn resumed_fork_with_reset_counter_adds_only_fork_usage() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a10ffe-fa34-7f33-9f68-56dad306d455";
        let sessions_dir = codex_home.join("sessions/2026/10/06");
        fs::create_dir_all(&sessions_dir).unwrap();
        let fork = sessions_dir.join(format!("rollout-2026-10-06T12-25-30-{session_id}.jsonl"));
        let continuation = sessions_dir.join(format!(
            "rollout-2026-10-06T13-00-00-{session_id}_01a11000-0000-7000-8000-000000000000.jsonl"
        ));
        write_codex_session_meta(
            &fork,
            session_id,
            Some("01a10ffd-846f-7191-beb4-4205e87a9d4b"),
        );
        write_codex_token_count_with_last(&fork, 103_916, 15_416);
        write_codex_token_count_with_last(&fork, 119_366, 15_450);
        write_codex_session_meta(&continuation, session_id, None);
        write_codex_token_count_with_last(&continuation, 10_000, 10_000);
        write_codex_token_count_with_last(&continuation, 20_000, 10_000);

        assert_eq!(
            codex_token_usage(&codex_home, session_id)["total_tokens"],
            50_866
        );

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn resumed_non_fork_keeps_full_usage() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a10fa1-901e-7141-bae3-6d61bcf8632a";
        let sessions_dir = codex_home.join("sessions/2026/10/06");
        fs::create_dir_all(&sessions_dir).unwrap();
        let original = sessions_dir.join(format!("rollout-2026-10-06T10-43-28-{session_id}.jsonl"));
        let continuation = sessions_dir.join(format!(
            "rollout-2026-10-06T10-44-41-{session_id}_01a10fa2-abb2-74f3-b82b-e5bcef3cc9ed.jsonl"
        ));
        write_codex_session_meta(&original, session_id, None);
        write_codex_token_count_with_last(&original, 26_492, 26_492);
        // Continuation files start above zero too, but that usage is the thread's own.
        write_codex_session_meta(&continuation, session_id, None);
        write_codex_token_count_with_last(&continuation, 133_315, 26_828);
        write_codex_token_count_with_last(&continuation, 248_538, 30_676);

        assert_eq!(
            codex_token_usage(&codex_home, session_id)["total_tokens"],
            248_538
        );

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn codex_rollout_without_token_count_reports_reason() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let session_id = "01a10fa1-91ac-7f71-a77b-9dd43e195e45";
        let sessions_dir = codex_home.join("sessions/2026/10/06");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(
            sessions_dir.join(format!("rollout-2026-10-06T10-43-28-{session_id}.jsonl")),
            "",
        )
        .unwrap();

        assert_eq!(
            codex_token_usage(&codex_home, session_id),
            json!({"available": false, "reason": "token_count_not_found"})
        );

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn missing_codex_rollout_reports_unavailable() {
        let codex_home = env::temp_dir().join(format!("tally-codex-home-{}", unique_suffix()));
        let usage = codex_token_usage(&codex_home, "missing-session");
        assert_eq!(
            usage,
            json!({"available": false, "reason": "rollout_not_found"})
        );
    }

    #[test]
    fn installs_and_restores_existing_desktop_notification() {
        let directory = env::temp_dir().join(format!("tally-notify-config-{}", unique_suffix()));
        let config_path = directory.join("config.toml");
        let state_dir = state_dir_for_config_path(&config_path);
        let binary_path = installed_binary_path_for_config_path(&config_path);
        let saved_path = previous_notify_path(&state_dir);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            &config_path,
            "# keep this comment\nmodel = \"gpt-test\"\nnotify = [\"keep-notify\", \"--flag\"]\n",
        )
        .unwrap();

        install_desktop_notify(&config_path, &binary_path, &state_dir, &saved_path, true).unwrap();
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

        install_desktop_notify(&config_path, &binary_path, &state_dir, &saved_path, true).unwrap();
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
    fn installs_turn_and_session_end_hooks() {
        assert!(EVENTS.iter().any(|event| event.name == "Stop"));
        assert!(EVENTS.iter().any(|event| event.name == "SessionEnd"));
    }

    #[test]
    fn installs_untrusted_config_hooks_without_changing_existing_trust() {
        let mut document = r#"model = "gpt-test"

[[hooks.SessionStart]]
matcher = ".*"

[[hooks.SessionStart.hooks]]
type = "command"
command = "echo keep"
timeout = 5

[hooks.state]

[hooks.state."existing-hook"]
trusted_hash = "sha256:keep"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let state_dir = Path::new("/tmp/tally state");
        install_tally_config_hooks(&mut document, "/tmp/tally-codex", state_dir).unwrap();

        let hooks = document["hooks"].as_table().unwrap();
        for event in EVENTS {
            let groups = hooks[event.name].as_array_of_tables().unwrap();
            assert_eq!(
                groups
                    .iter()
                    .flat_map(|group| group["hooks"].as_array_of_tables().unwrap().iter())
                    .filter(|handler| {
                        handler["command"]
                            .as_str()
                            .is_some_and(is_tally_hook_command)
                    })
                    .count(),
                1
            );
        }
        assert_eq!(
            hooks["state"]["existing-hook"]["trusted_hash"].as_str(),
            Some("sha256:keep")
        );
        assert_eq!(document.to_string().matches("trusted_hash").count(), 1);

        assert_eq!(
            remove_tally_config_hooks(&mut document).unwrap(),
            EVENTS.len()
        );
        let hooks = document["hooks"].as_table().unwrap();
        assert_eq!(
            hooks["state"]["existing-hook"]["trusted_hash"].as_str(),
            Some("sha256:keep")
        );
        assert!(document.to_string().contains("command = \"echo keep\""));
        assert!(!document.to_string().contains("tally-codex"));
    }

    #[test]
    fn uses_tool_call_id_to_correlate_actions_and_results() {
        let pre = json!({"tool_call_id": "tool-123", "arguments": {"command": "true"}});
        let post = json!({"tool_call_id": "tool-123", "result": {"stdout": ""}});
        assert_eq!(action_id(&pre), "act_tool-123");
        assert_eq!(action_id(&pre), action_id(&post));
    }

    #[test]
    fn safe_slug_has_fallback_and_limits_length() {
        assert_eq!(safe_slug("hello world!", "x"), "hello_world");
        assert_eq!(safe_slug("!!!", "fallback"), "fallback");
        assert!(safe_slug(&"a".repeat(200), "x").len() <= 96);
    }
}
