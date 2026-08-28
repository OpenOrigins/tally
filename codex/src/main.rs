use fs2::FileExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
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

    if args.first().map(String::as_str) == Some("exec")
        && env_enabled("TALLY_TEE_CODEX_STDIO", false)
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
    set_runtime_defaults();

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
    set_runtime_defaults();
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
        installed_binary_path_for_config_path(path),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
