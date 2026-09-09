use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::Result;

pub use super::server_evidence::{evidence_summary, server_evidence};

pub struct AuditSinkConfig<'a> {
    pub source: &'a str,
    pub log_root: PathBuf,
    pub run_id: String,
    pub workspace: PathBuf,
    pub forwarding_state_dir: PathBuf,
    pub agent_id: String,
    pub heartbeat_client: &'a str,
    pub event_schema: &'a str,
}

pub struct AuditSink {
    pub source: String,
    pub run_id: String,
    pub workspace: PathBuf,
    pub state_dir: PathBuf,
    log_root: PathBuf,
    jsonl_dir: PathBuf,
    private_dir: PathBuf,
    forwarding_state_dir: PathBuf,
    agent_id: String,
    heartbeat_client: String,
    event_schema: String,
    staged_private_objects: RefCell<BTreeMap<PathBuf, Value>>,
}

impl AuditSink {
    pub fn new(config: AuditSinkConfig<'_>) -> Result<Self> {
        let source = safe_slug(config.source, "source");
        let log_root = config.log_root;
        super::mark_tally_data_directory(&log_root)?;
        let jsonl_dir = log_root.join("jsonl");
        let private_dir = log_root.join("private").join("objects");
        let state_dir = log_root.join("state");
        for path in [&jsonl_dir, &private_dir, &state_dir] {
            super::create_private_dir(path)?;
        }
        Ok(Self {
            source,
            run_id: config.run_id,
            workspace: config.workspace,
            state_dir,
            log_root,
            jsonl_dir,
            private_dir,
            forwarding_state_dir: config.forwarding_state_dir,
            agent_id: config.agent_id,
            heartbeat_client: config.heartbeat_client.to_string(),
            event_schema: config.event_schema.to_string(),
            staged_private_objects: RefCell::new(BTreeMap::new()),
        })
    }

    pub fn private_payload(&self, payload: &Value) -> Result<Value> {
        let hash = sha256_value(payload);
        let digest = hash.trim_start_matches("sha256:");
        let path = self
            .private_dir
            .join(&digest[..2])
            .join(format!("{digest}.json"));
        self.staged_private_objects
            .borrow_mut()
            .entry(path)
            .or_insert_with(|| payload.clone());
        Ok(json!({
            "hash": hash,
            "uri": format!("private://sha256/{digest}"),
        }))
    }

    pub fn append_jsonl(&self, stream_name: &str, event: &Value) -> Result<()> {
        if !env_enabled("TALLY_DEBUG_JSONL", false) {
            return Ok(());
        }
        append_jsonl_locked(
            &self
                .jsonl_dir
                .join(format!("{}.jsonl", safe_slug(stream_name, "stream"))),
            event,
        )
    }

    pub fn write_tally_record(&self, record: &Value) -> Result<u64> {
        let mut with_defaults = record.clone();
        with_defaults["run_id"] = Value::String(self.run_id.clone());
        if with_defaults.get("schema_version").is_none() {
            with_defaults["schema_version"] = Value::String("0.2".to_string());
        }
        let private_paths = super::private_paths_for_record(&self.log_root, &with_defaults);
        let private_objects = self
            .staged_private_objects
            .borrow()
            .iter()
            .map(|(path, value)| (path.clone(), value.clone()))
            .collect::<Vec<_>>();
        let sequence = super::journal::append_record(
            &self.forwarding_state_dir,
            &with_defaults,
            Some(&self.log_root),
            &private_paths,
            &private_objects,
        )?;
        self.staged_private_objects.borrow_mut().clear();
        let executable = env::current_exe()?;
        if let Err(error) = super::schedule_forwarder(&self.forwarding_state_dir, &executable) {
            eprintln!(
                "Warning: record was journaled but the delivery worker could not start: {error}"
            );
        }
        Ok(sequence)
    }

    pub fn emit_heartbeat(
        &self,
        active_sessions: &[String],
        stream_name: &str,
        mut metadata: Value,
        requested_interval_seconds: u64,
    ) -> Result<()> {
        let emitted_at_unix_millis = unix_now_millis();
        let Some(rate_limit_seconds) = super::claim_agent_heartbeat(
            &self.state_dir,
            &self.agent_id,
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
                "agent_id": self.agent_id,
                "client": self.heartbeat_client,
                "emitted_at_unix_millis": emitted_at_unix_millis,
            }),
        );
        let mut event = json!({
            "schema_version": self.event_schema,
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
            "agent_id": self.agent_id,
            "anchor_instance_id": stable_id(
                "anchor",
                &Value::String(self.forwarding_state_dir.display().to_string()),
            ),
            "active_sessions": active_sessions,
            "timestamp": timestamp,
            "source": self.source,
            "metadata": metadata,
        }))?;
        super::commit_agent_heartbeat(&self.state_dir, &self.agent_id, emitted_at_unix_millis)?;
        Ok(())
    }
}

pub struct HeartbeatFiles {
    state: PathBuf,
    pid: PathBuf,
    status: PathBuf,
}

impl HeartbeatFiles {
    pub fn new(log_root: &Path, run_id: &str) -> Self {
        let state_dir = log_root.join("state");
        let run_id = safe_slug(run_id, "run");
        Self {
            state: state_dir.join(format!("hook-heartbeat.{run_id}.json")),
            pid: state_dir.join(format!("hook-heartbeat.{run_id}.pid")),
            status: state_dir.join(format!("hook-heartbeat.{run_id}.daemon.json")),
        }
    }
}

pub fn update_heartbeat_state(
    sink: &AuditSink,
    files: &HeartbeatFiles,
    client_command: &str,
    event_type: &str,
    session_id: Option<String>,
    observed_at: &str,
) -> Result<()> {
    if !env_enabled("TALLY_HOOK_HEARTBEAT_ENABLED", true) {
        return Ok(());
    }

    write_json_atomic(
        &files.state,
        &json!({
            "run_id": sink.run_id,
            "session_id": session_id.unwrap_or_else(|| sink.run_id.clone()),
            "updated_at": observed_at,
            "last_hook_event": event_type,
            "stop_requested": heartbeat_stop_requested(event_type),
        }),
    )?;
    super::record_agent_activity(&sink.state_dir, &sink.agent_id, unix_now_millis())?;

    if event_type != "SessionStart" {
        return Ok(());
    }

    super::spawn_background(&env::current_exe()?, &[client_command, "heartbeat-daemon"])?;
    Ok(())
}

pub fn run_heartbeat_daemon(sink: &AuditSink, files: &HeartbeatFiles) -> Result<()> {
    let Some(pid_file) = claim_heartbeat_daemon(&files.pid)? else {
        return Ok(());
    };
    write_daemon_status(&files.status, "started", &files.state)?;

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
        let state = match read_json_file(&files.state) {
            Ok(state) => state,
            Err(error) => {
                eprintln!(
                    "Warning: stopping heartbeat daemon because its state is unreadable: {error}"
                );
                break;
            }
        };
        if state["stop_requested"].as_bool().unwrap_or(false) {
            break;
        }
        let quiet_seconds = match validated_heartbeat_quiet_seconds(&state) {
            Ok(seconds) => seconds,
            Err(error) => {
                eprintln!("Warning: stopping heartbeat daemon because {error}");
                break;
            }
        };
        let heartbeat_kind = if quiet_seconds > idle_timeout {
            "hook-daemon-timeout"
        } else if heartbeat_due(quiet_seconds, interval) {
            "hook-daemon"
        } else {
            continue;
        };
        sink.emit_heartbeat(
            &[state["session_id"]
                .as_str()
                .unwrap_or(&sink.run_id)
                .to_string()],
            "hook-heartbeat",
            json!({
                "heartbeat_kind": heartbeat_kind,
                "last_hook_event": state["last_hook_event"],
                "last_hook_observed_at": state["updated_at"],
            }),
            interval,
        )?;
        if heartbeat_kind == "hook-daemon-timeout" {
            break;
        }
    }

    write_daemon_status(&files.status, "stopped", &files.state)?;
    FileExt::unlock(&pid_file)?;
    if let Err(error) = remove_file_if_exists(&files.pid) {
        eprintln!("Warning: could not remove the heartbeat PID file: {error}");
    }
    Ok(())
}

fn write_daemon_status(path: &Path, status: &str, state_path: &Path) -> Result<()> {
    write_json_atomic(
        path,
        &json!({
            "pid": std::process::id(),
            "status": status,
            "updated_at": utc_now(),
            "state_path": state_path,
        }),
    )
}

fn claim_heartbeat_daemon(path: &Path) -> io::Result<Option<fs::File>> {
    if let Some(parent) = path.parent() {
        super::create_private_dir(parent)?;
    }
    let mut file = match private_open_options()
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

fn validated_heartbeat_quiet_seconds(state: &Value) -> Result<i64> {
    let quiet_seconds = state["updated_at"]
        .as_str()
        .and_then(seconds_since)
        .ok_or("updated_at is missing or invalid")?;
    if quiet_seconds < -300 {
        return Err("updated_at is too far in the future".into());
    }
    Ok(quiet_seconds)
}

fn heartbeat_stop_requested(event_type: &str) -> bool {
    event_type == "SessionEnd"
}

fn configured_heartbeat_interval() -> u64 {
    super::heartbeat_interval_seconds(env_u64(
        "TALLY_HOOK_HEARTBEAT_SECONDS",
        env_u64(
            "TALLY_HEARTBEAT_SECONDS",
            super::DEFAULT_HEARTBEAT_INTERVAL_SECONDS,
        ),
    ))
}

pub fn read_stdin() -> Result<String> {
    let limit =
        env_u64("TALLY_MAX_HOOK_INPUT_BYTES", 16 * 1024 * 1024).clamp(64 * 1024, 64 * 1024 * 1024);
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(format!("hook input exceeded the configured {limit}-byte limit").into());
    }
    Ok(String::from_utf8(bytes)?)
}

pub fn parse_payload(raw: &str) -> Value {
    if raw.is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw).unwrap_or_else(|_| json!({"raw_stdin": raw}))
    }
}

pub fn first_string_by_key(value: &Value, names: &[&str]) -> Option<String> {
    let mut stack = vec![(value, 0_usize)];
    let mut visited = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        visited += 1;
        if visited > 100_000 || depth > 64 {
            continue;
        }
        match value {
            Value::Object(map) => {
                for name in names {
                    if let Some(value) = map
                        .get(*name)
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    {
                        return Some(value.to_string());
                    }
                }
                stack.extend(map.values().rev().map(|value| (value, depth + 1)));
            }
            Value::Array(items) => stack.extend(items.iter().rev().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    None
}

pub fn first_mapping_by_key<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    let mut stack = vec![(value, 0_usize)];
    let mut visited = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        visited += 1;
        if visited > 100_000 || depth > 64 {
            continue;
        }
        match value {
            Value::Object(map) => {
                for name in names {
                    if let Some(value) = map.get(*name).filter(|value| value.is_object()) {
                        return Some(value);
                    }
                }
                stack.extend(map.values().rev().map(|value| (value, depth + 1)));
            }
            Value::Array(items) => stack.extend(items.iter().rev().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    None
}

pub fn first_value_by_key<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    let mut stack = vec![(value, 0_usize)];
    let mut visited = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        visited += 1;
        if visited > 100_000 || depth > 64 {
            continue;
        }
        match value {
            Value::Object(map) => {
                for name in names {
                    if let Some(value) = map.get(*name).filter(|value| !value.is_null()) {
                        return Some(value);
                    }
                }
                stack.extend(map.values().rev().map(|value| (value, depth + 1)));
            }
            Value::Array(items) => stack.extend(items.iter().rev().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    None
}

pub fn stable_id(prefix: &str, value: &Value) -> String {
    let hash = sha256_value(value);
    format!(
        "{}_{}",
        safe_slug(prefix, "id"),
        hash.trim_start_matches("sha256:")
            .chars()
            .take(32)
            .collect::<String>()
    )
}

pub fn light_git_state(cwd: &Path) -> Value {
    if !cwd.ancestors().any(|path| path.join(".git").exists()) {
        return json!({
            "is_git_repo": false,
            "workspace": cwd.display().to_string(),
        });
    }
    let untracked = if env_enabled("TALLY_GIT_INCLUDE_UNTRACKED", false) {
        "--untracked-files=normal"
    } else {
        "--untracked-files=no"
    };
    let status = run_command(
        &["git", "status", "--porcelain=v2", "--branch", untracked],
        cwd,
    );
    if status["exit_code"].as_i64() != Some(0) {
        return json!({
            "is_git_repo": false,
            "workspace": cwd.display().to_string(),
            "capture": status,
        });
    }
    let status_stdout = status["stdout"].as_str().unwrap_or("");
    let head = status_stdout
        .lines()
        .find_map(|line| line.strip_prefix("# branch.oid "))
        .unwrap_or("");
    let branch = status_stdout
        .lines()
        .find_map(|line| line.strip_prefix("# branch.head "))
        .unwrap_or("");
    json!({
        "is_git_repo": true,
        "workspace": cwd.display().to_string(),
        "head": head,
        "branch": branch,
        "status": tail_chars(status_stdout, 20_000),
        "status_hash": sha256_str(status_stdout),
        "untracked_files_included": env_enabled("TALLY_GIT_INCLUDE_UNTRACKED", false),
    })
}

fn run_command(argv: &[&str], cwd: &Path) -> Value {
    let started = SystemTime::now();
    let argv_vec = argv
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let stdout_path = env::temp_dir().join(format!("tally-command-{}.stdout", unique_suffix()));
    let stderr_path = env::temp_dir().join(format!("tally-command-{}.stderr", unique_suffix()));
    let stdout = private_open_options()
        .create_new(true)
        .write(true)
        .open(&stdout_path);
    let stderr = private_open_options()
        .create_new(true)
        .write(true)
        .open(&stderr_path);
    let (stdout, stderr) = match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => (stdout, stderr),
        (Err(error), _) | (_, Err(error)) => {
            return json!({
                "argv": argv_vec,
                "exit_code": Value::Null,
                "duration_ms": started.elapsed().map(|duration| duration.as_millis()).unwrap_or(0),
                "error": error.to_string(),
            })
        }
    };
    let mut child = match Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_file(&stdout_path);
            let _ = fs::remove_file(&stderr_path);
            return json!({
                "argv": argv_vec,
                "exit_code": Value::Null,
                "duration_ms": started.elapsed().map(|duration| duration.as_millis()).unwrap_or(0),
                "error": error.to_string(),
            });
        }
    };
    let timeout =
        Duration::from_millis(env_u64("TALLY_GIT_TIMEOUT_MILLIS", 2_000).clamp(100, 10_000));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                timed_out = true;
                let _ = child.kill();
                break child.wait();
            }
            Err(error) => break Err(error),
        }
    };
    let stdout = fs::read(&stdout_path).unwrap_or_default();
    let stderr = fs::read(&stderr_path).unwrap_or_default();
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    match status {
        Ok(status) => json!({
            "argv": argv_vec,
            "exit_code": status.code(),
            "duration_ms": started.elapsed().map(|duration| duration.as_millis()).unwrap_or(0),
            "stdout": tail_chars(&String::from_utf8_lossy(&stdout), 20_000),
            "stderr": tail_chars(&String::from_utf8_lossy(&stderr), 20_000),
            "timed_out": timed_out,
        }),
        Err(error) => json!({
            "argv": argv_vec,
            "exit_code": Value::Null,
            "duration_ms": started.elapsed().map(|duration| duration.as_millis()).unwrap_or(0),
            "error": error.to_string(),
        }),
    }
}

fn tail_chars(value: &str, max: usize) -> String {
    let chars = value.chars().rev().take(max).collect::<Vec<_>>();
    chars.into_iter().rev().collect()
}

fn append_jsonl_locked(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        super::create_private_dir(parent)?;
    }
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    let lock = private_open_options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    let mut file = private_open_options()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(value)?)?;
    FileExt::unlock(&lock)?;
    Ok(())
}

pub fn read_json_file(path: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn private_open_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let mut options = options;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    let contents = format!("{}\n", serde_json::to_string_pretty(value)?);
    write_text_atomic(path, &contents)
}

pub fn write_text_atomic(path: &Path, value: &str) -> Result<()> {
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .unwrap_or(0o600)
    };
    #[cfg(not(unix))]
    let mode = 0o600;
    super::atomic_write(path, value.as_bytes(), mode)?;
    Ok(())
}

pub fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn sha256_value(value: &Value) -> String {
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut writer = HashWriter(Sha256::new());
    if serde_json::to_writer(&mut writer, value).is_err() {
        return sha256_bytes(&[]);
    }
    format!("sha256:{:x}", writer.0.finalize())
}

pub fn sha256_str(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

pub fn merge_object(target: &mut Value, extra: Value) {
    let Some(target) = target.as_object_mut() else {
        return;
    };
    if let Value::Object(extra) = extra {
        target.extend(extra);
    }
}

pub fn safe_slug(value: &str, default: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
            output.push(character);
        } else if !output.ends_with('_') {
            output.push('_');
        }
        if output.len() >= 96 {
            break;
        }
    }
    let output = output.trim_matches('_');
    if output.is_empty() {
        default.to_string()
    } else {
        output.to_string()
    }
}

pub fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn unix_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn backup_stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%S%.9fZ").to_string()
}

pub fn random_hex(bytes: usize) -> String {
    let mut input = vec![0_u8; bytes];
    if getrandom::fill(&mut input).is_err() {
        input = format!(
            "{}-{}-{}",
            std::process::id(),
            unix_now_millis(),
            env::var("USER").unwrap_or_default()
        )
        .into_bytes();
    }
    let hash = Sha256::digest(&input);
    hash.iter()
        .take(bytes)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn secure_random_hex(bytes: usize) -> Result<String> {
    let mut input = vec![0_u8; bytes];
    getrandom::fill(&mut input)
        .map_err(|error| format!("secure randomness unavailable: {error}"))?;
    Ok(input.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn seconds_since(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| (Utc::now() - time.with_timezone(&Utc)).num_seconds())
        .ok()
}

#[cfg(unix)]
pub fn process_is_alive(pid: &str) -> bool {
    pid.parse::<i32>()
        .ok()
        .is_some_and(|pid| unsafe { libc::kill(pid, 0) == 0 })
}

#[cfg(windows)]
pub fn process_is_alive(pid: &str) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    pid.parse::<u32>().ok().is_some_and(|pid| unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            false
        } else {
            CloseHandle(handle);
            true
        }
    })
}

#[cfg(all(not(unix), not(windows)))]
pub fn process_is_alive(_pid: &str) -> bool {
    false
}

#[cfg(unix)]
pub fn hook_command(hook_bin: &str, client: &str, event_name: &str, state_dir: &Path) -> String {
    format!(
        "TALLY_STATE_DIR={} {} {} hook {}",
        shell_quote(&state_dir.display().to_string()),
        shell_quote(hook_bin),
        shell_quote(client),
        shell_quote(event_name)
    )
}

#[cfg(windows)]
pub fn hook_command(hook_bin: &str, client: &str, event_name: &str, state_dir: &Path) -> String {
    format!(
        "set \"TALLY_STATE_DIR={}\" && {} {} hook {}",
        state_dir.display(),
        shell_quote(hook_bin),
        shell_quote(client),
        shell_quote(event_name)
    )
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "_+-./:".contains(character))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(windows)]
fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "_+-./:\\".contains(character))
    {
        value.to_string()
    } else {
        format!("\"{value}\"")
    }
}

pub fn unique_suffix() -> String {
    format!("{}-{}", std::process::id(), random_hex(4))
}

pub fn home_dir() -> String {
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

pub fn workspace_path() -> PathBuf {
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

pub fn run_id() -> String {
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

pub fn expand_home(value: &str) -> PathBuf {
    if value == "~" {
        return PathBuf::from(home_dir());
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return PathBuf::from(home_dir()).join(rest);
    }
    PathBuf::from(value)
}

pub fn backup_if_exists(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("cannot back up unnamed path {}", path.display()))?;
    let backup = path.with_file_name(format!("{filename}.backup-{}", backup_stamp()));
    fs::copy(path, &backup)?;
    Ok(Some(backup))
}

pub fn env_enabled(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(value) => !matches!(value.as_str(), "0" | "false" | "False" | "no"),
        Err(_) => default,
    }
}

pub fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

pub fn set_default(key: &str, value: &str) {
    if env::var(key).unwrap_or_default().is_empty() {
        env::set_var(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        claim_heartbeat_daemon, first_string_by_key, heartbeat_due, heartbeat_stop_requested,
        parse_payload, safe_slug, sha256_str, stable_id, unique_suffix,
        validated_heartbeat_quiet_seconds, AuditSink, AuditSinkConfig,
    };
    use fs2::FileExt;
    use serde_json::json;
    use std::{env, fs};

    #[test]
    fn parses_payload_and_finds_nested_strings() {
        assert!(parse_payload("")
            .as_object()
            .is_some_and(|value| value.is_empty()));
        assert_eq!(parse_payload("not-json"), json!({"raw_stdin": "not-json"}));
        assert_eq!(
            first_string_by_key(
                &json!({"outer": {"session_id": "session-1"}}),
                &["session_id"]
            ),
            Some("session-1".to_string())
        );
    }

    #[test]
    fn stable_identifiers_are_deterministic_and_safe() {
        let payload = json!({"value": 1});
        assert_eq!(stable_id("event", &payload), stable_id("event", &payload));
        assert!(sha256_str("value").starts_with("sha256:"));
        assert_eq!(safe_slug("hello world!", "fallback"), "hello_world");
        assert_eq!(safe_slug("!!!", "fallback"), "fallback");
    }

    #[test]
    fn private_payloads_stage_one_content_addressed_object() {
        let directory = env::temp_dir().join(format!("tally-objects-{}", unique_suffix()));
        let log_root = directory.join("logs");
        let forwarding_state_dir = directory.join("forwarding");
        let sink = AuditSink::new(AuditSinkConfig {
            source: "test",
            log_root: log_root.clone(),
            run_id: "run-a".to_string(),
            workspace: directory.clone(),
            forwarding_state_dir,
            agent_id: "agent-a".to_string(),
            heartbeat_client: "test",
            event_schema: "test.v1",
        })
        .unwrap();
        let first = sink.private_payload(&json!({"same": true})).unwrap();
        let second = sink.private_payload(&json!({"same": true})).unwrap();

        assert_eq!(first["hash"], second["hash"]);
        assert_eq!(first["uri"], second["uri"]);
        assert!(first["uri"]
            .as_str()
            .unwrap()
            .starts_with("private://sha256/"));
        assert_eq!(sink.staged_private_objects.borrow().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn heartbeat_waits_for_quiet_and_stops_only_at_session_end() {
        assert!(!heartbeat_due(0, 600));
        assert!(!heartbeat_due(599, 600));
        assert!(heartbeat_due(600, 600));
        assert!(heartbeat_due(1_200, 600));
        assert!(!heartbeat_stop_requested("Stop"));
        assert!(heartbeat_stop_requested("SessionEnd"));
    }

    #[test]
    fn heartbeat_rejects_corrupt_and_future_state() {
        assert!(validated_heartbeat_quiet_seconds(&json!({}))
            .unwrap_err()
            .to_string()
            .contains("missing or invalid"));
        assert!(
            validated_heartbeat_quiet_seconds(&json!({"updated_at": "not-a-timestamp"}))
                .unwrap_err()
                .to_string()
                .contains("missing or invalid")
        );
        assert!(
            validated_heartbeat_quiet_seconds(&json!({"updated_at": "2099-01-01T00:00:00Z"}))
                .unwrap_err()
                .to_string()
                .contains("too far in the future")
        );
    }

    #[test]
    fn heartbeat_daemon_has_one_owner() {
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

    #[cfg(windows)]
    #[test]
    fn windows_hook_command_quotes_paths_but_not_fixed_arguments() {
        let command = super::hook_command(
            r"C:\Program Files\Tally\tally.exe",
            "codex",
            "SessionEnd",
            std::path::Path::new(r"C:\Users\Test User\.codex\tally\logs\.state"),
        );
        assert!(command.contains(r#""C:\Program Files\Tally\tally.exe""#));
        assert!(command.ends_with(" codex hook SessionEnd"));
    }
}
