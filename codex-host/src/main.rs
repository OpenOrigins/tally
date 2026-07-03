use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
        "Tally: recording Codex Desktop session start",
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
    HookEvent::new("Stop", None, "Tally: recording Codex Desktop stop"),
];

fn main() {
    if let Err(error) = run() {
        eprintln!("tally-host-hook: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match env::args().nth(1).as_deref() {
        Some("install") => install_hooks(),
        Some("uninstall") => uninstall_hooks(),
        Some("heartbeat-daemon") => run_heartbeat_daemon(),
        Some(event_name) => record_hook_event(event_name),
        None => record_hook_event("unknown"),
    }
}

fn record_hook_event(event_name: &str) -> Result<()> {
    let raw_payload = read_stdin()?;
    let payload = parse_payload(&raw_payload);
    let run_id = run_id_from_env_or_payload(&payload);
    let session_id = session_id(&payload).unwrap_or_else(|| run_id.clone());
    let sink = Sink::new("codex-hooks", &run_id)?;
    let observed_at = now();
    let payload_hash = sha256(raw_payload.as_bytes());

    let event = json!({
        "schema_version": "tally-codex-host.v1",
        "run_id": run_id,
        "session_id": session_id,
        "source": "codex-host-hook",
        "event_type": event_name,
        "observed_at": observed_at,
        "payload_hash": payload_hash,
        "payload": payload,
    });
    sink.append_jsonl("codex-hooks", &event)?;
    sink.write_record(&tally_record(event_name, &event))?;
    update_heartbeat(&sink, event_name, &event)?;
    Ok(())
}

fn update_heartbeat(sink: &Sink, event_name: &str, event: &Value) -> Result<()> {
    if !heartbeat_enabled() {
        return Ok(());
    }

    let state = json!({
        "run_id": &sink.run_id,
        "session_id": event["session_id"],
        "updated_at": event["observed_at"],
        "last_event": event_name,
        "stop_requested": event_name == "Stop",
    });
    write_json_atomic(&heartbeat_state_path(&sink.run_id), &state)?;
    write_heartbeat(sink, "hook-event", event_name)?;

    if event_name == "SessionStart" && !heartbeat_daemon_running(&sink.run_id) {
        let mut command = Command::new(env::current_exe()?);
        command
            .arg("heartbeat-daemon")
            .env("TALLY_RUN_ID", &sink.run_id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn()?;
    }

    Ok(())
}

fn run_heartbeat_daemon() -> Result<()> {
    let run_id = run_id_from_env();
    let sink = Sink::new("hook-heartbeat", &run_id)?;
    let pid_path = heartbeat_pid_path(&run_id);
    fs::write(&pid_path, std::process::id().to_string())?;

    let interval = env_u64("TALLY_HOOK_HEARTBEAT_SECONDS", 60);
    let idle_limit = env_u64("TALLY_HOOK_HEARTBEAT_IDLE_SECONDS", 300);

    loop {
        let state = read_json(&heartbeat_state_path(&run_id)).unwrap_or_else(|_| json!({}));
        if state["stop_requested"].as_bool().unwrap_or(false) {
            break;
        }
        if heartbeat_idle_seconds(&state) > idle_limit {
            write_heartbeat(&sink, "hook-daemon-timeout", "timeout")?;
            break;
        }
        write_heartbeat(
            &sink,
            "hook-daemon",
            state["last_event"].as_str().unwrap_or("unknown"),
        )?;
        thread::sleep(Duration::from_secs(interval));
    }

    let _ = fs::remove_file(pid_path);
    Ok(())
}

fn write_heartbeat(sink: &Sink, heartbeat_kind: &str, last_event: &str) -> Result<()> {
    let record = json!({
        "record_type": "HEARTBEAT",
        "schema_version": "0.2-mvp",
        "run_id": &sink.run_id,
        "session_id": &sink.run_id,
        "agent_id": agent_id(),
        "timestamp": now(),
        "source": &sink.source,
        "metadata": {
            "heartbeat_kind": heartbeat_kind,
            "last_event": last_event,
        },
    });
    sink.append_jsonl("hook-heartbeat", &record)?;
    sink.write_record(&record)?;
    Ok(())
}

fn tally_record(event_name: &str, event: &Value) -> Value {
    let record_type = match event_name {
        "SessionStart" => "SESSION_START",
        "UserPromptSubmit" => "INSTRUCTION_RECEIVED",
        "PreToolUse" | "PermissionRequest" => "ACTION_TAKEN",
        "PostToolUse" => "RESULT_RECEIVED",
        "Stop" => "SESSION_END",
        _ => "CODEX_LIFECYCLE",
    };

    json!({
        "record_type": record_type,
        "schema_version": "0.2-mvp",
        "record_id": format!(
            "rec_{}",
            short_hash(event["payload_hash"].as_str().unwrap_or("missing-payload-hash"))
        ),
        "run_id": event["run_id"],
        "session_id": event["session_id"],
        "agent_id": agent_id(),
        "agent_version": agent_version(),
        "codex_hook_event": event_name,
        "observed_at": event["observed_at"],
        "payload_hash": event["payload_hash"],
        "payload_uri": "jsonl://codex-hooks",
    })
}

fn install_hooks() -> Result<()> {
    let hook_binary = env::current_exe()?;
    let path = hooks_path();
    fs::create_dir_all(log_root())?;

    let mut config = if path.exists() {
        read_json(&path)?
    } else {
        json!({"hooks": {}})
    };

    let hooks = config
        .as_object_mut()
        .ok_or("hooks.json must contain a JSON object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        return Err("hooks.json field `hooks` must be a JSON object".into());
    }

    let backup = backup_if_exists(&path)?;
    remove_tally_handlers(&mut config);
    let hooks = config["hooks"].as_object_mut().expect("checked above");
    for event in EVENTS {
        hooks
            .entry(event.name.to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{} must be a JSON array", event.name))?
            .push(event.to_hook_group(&hook_binary));
    }

    write_json_atomic(&path, &config)?;
    println!("Installed Tally Codex hooks into {}", path.display());
    if let Some(backup) = backup {
        println!("Backed up previous hooks file to {}", backup.display());
    }
    println!("Hook binary: {}", hook_binary.display());
    println!("Logs: {}", log_root().display());
    Ok(())
}

fn uninstall_hooks() -> Result<()> {
    let path = hooks_path();
    if !path.exists() {
        println!("No hooks file found at {}", path.display());
        return Ok(());
    }

    let mut config = read_json(&path)?;
    let backup = backup_if_exists(&path)?;
    let removed = remove_tally_handlers(&mut config);
    write_json_atomic(&path, &config)?;

    println!(
        "Removed {removed} Tally hook handler(s) from {}",
        path.display()
    );
    if let Some(backup) = backup {
        println!("Backed up previous hooks file to {}", backup.display());
    }
    Ok(())
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

    fn to_hook_group(self, hook_binary: &Path) -> Value {
        let mut group = serde_json::Map::new();
        if let Some(matcher) = self.matcher {
            group.insert("matcher".to_string(), json!(matcher));
        }
        group.insert(
            "hooks".to_string(),
            json!([{
                "type": "command",
                "command": format!("{} {}", shell_quote(&hook_binary.display().to_string()), self.name),
                "timeout": 15,
                "statusMessage": self.status,
            }]),
        );
        Value::Object(group)
    }
}

struct Sink {
    source: String,
    run_id: String,
    root: PathBuf,
}

impl Sink {
    fn new(source: &str, run_id: &str) -> Result<Self> {
        let sink = Self {
            source: safe_slug(source),
            run_id: safe_slug(run_id),
            root: log_root(),
        };
        fs::create_dir_all(sink.root.join("jsonl"))?;
        fs::create_dir_all(sink.root.join("state"))?;
        fs::create_dir_all(sink.root.join("tally").join(&sink.source))?;
        Ok(sink)
    }

    fn append_jsonl(&self, stream: &str, value: &Value) -> Result<()> {
        append_jsonl_locked(
            &self
                .root
                .join("jsonl")
                .join(format!("{}.jsonl", safe_slug(stream))),
            value,
        )
    }

    fn write_record(&self, value: &Value) -> Result<()> {
        let record_type = value["record_type"].as_str().unwrap_or("RECORD");
        let record_id = value["record_id"]
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("rec_{}", short_hash(&sha256(now().as_bytes()))));
        let path = self.root.join("tally").join(&self.source).join(format!(
            "{}_{}_{}_{}.json",
            timestamp_for_filename(),
            safe_slug(record_type),
            safe_slug(&record_id),
            unique_suffix()
        ));
        write_json_atomic(&path, value)
    }
}

fn remove_tally_handlers(config: &mut Value) -> usize {
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };

    let mut removed = 0;
    let mut empty_events = Vec::new();

    for (event_name, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };

        groups.retain_mut(|group| {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|handler| {
                !handler["command"]
                    .as_str()
                    .map(|command| command.contains("tally-host-hook"))
                    .unwrap_or(false)
            });
            removed += before - handlers.len();
            !handlers.is_empty()
        });

        if groups.is_empty() {
            empty_events.push(event_name.clone());
        }
    }

    for event_name in empty_events {
        hooks.remove(&event_name);
    }

    removed
}

fn read_stdin() -> Result<String> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    Ok(text)
}

fn parse_payload(raw: &str) -> Value {
    if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw).unwrap_or_else(|_| json!({"raw": raw}))
    }
}

fn session_id(value: &Value) -> Option<String> {
    first_string(
        value,
        &[
            "session_id",
            "thread_id",
            "conversation_id",
            "conversationId",
        ],
    )
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(Value::String(value)) = map.get(*key) {
                    if !value.is_empty() {
                        return Some(value.clone());
                    }
                }
            }
            map.values().find_map(|value| first_string(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| first_string(value, keys)),
        _ => None,
    }
}

fn run_id_from_env_or_payload(payload: &Value) -> String {
    env::var("TALLY_RUN_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| session_id(payload).map(|value| format!("codex_{value}")))
        .unwrap_or_else(|| "codex_desktop".to_string())
}

fn run_id_from_env() -> String {
    env::var("TALLY_RUN_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "codex_desktop".to_string())
}

fn heartbeat_enabled() -> bool {
    !matches!(
        env::var("TALLY_HOOK_HEARTBEAT_ENABLED")
            .unwrap_or_else(|_| "1".to_string())
            .as_str(),
        "0" | "false" | "False" | "no"
    )
}

fn heartbeat_daemon_running(run_id: &str) -> bool {
    let Ok(pid) = fs::read_to_string(heartbeat_pid_path(run_id)) else {
        return false;
    };
    Command::new("kill")
        .arg("-0")
        .arg(pid.trim())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn heartbeat_idle_seconds(state: &Value) -> u64 {
    let Some(updated_at) = state["updated_at"].as_str() else {
        return 0;
    };
    let Ok(updated_at) = chrono::DateTime::parse_from_rfc3339(updated_at) else {
        return 0;
    };
    (Utc::now() - updated_at.with_timezone(&Utc))
        .num_seconds()
        .max(0) as u64
}

fn heartbeat_state_path(run_id: &str) -> PathBuf {
    log_root()
        .join("state")
        .join(format!("heartbeat.{}.json", safe_slug(run_id)))
}

fn heartbeat_pid_path(run_id: &str) -> PathBuf {
    log_root()
        .join("state")
        .join(format!("heartbeat.{}.pid", safe_slug(run_id)))
}

fn hooks_path() -> PathBuf {
    env::var("CODEX_HOOKS_PATH")
        .map(expand_home)
        .unwrap_or_else(|_| codex_home().join("hooks.json"))
}

fn codex_home() -> PathBuf {
    env::var("CODEX_HOME")
        .map(expand_home)
        .unwrap_or_else(|_| home().join(".codex"))
}

fn log_root() -> PathBuf {
    env::var("TALLY_LOG_ROOT")
        .map(expand_home)
        .unwrap_or_else(|_| home().join(".tally-codex").join("logs"))
}

fn home() -> PathBuf {
    env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn expand_home(path: String) -> PathBuf {
    if path == "~" {
        home()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(path)
    }
}

fn agent_id() -> String {
    env::var("TALLY_AGENT_ID").unwrap_or_else(|_| "codex-desktop".to_string())
}

fn agent_version() -> String {
    env::var("TALLY_AGENT_VERSION").unwrap_or_else(|_| "codex-app".to_string())
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn backup_if_exists(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let backup = path.with_file_name(format!(
        "{}.backup-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    fs::copy(path, &backup)?;
    Ok(Some(backup))
}

fn read_json(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        short_hash(&sha256(now().as_bytes()))
    ));
    fs::write(&tmp, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn append_jsonl_locked(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .append(true)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(value)?)?;
    lock.unlock()?;
    Ok(())
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn timestamp_for_filename() -> String {
    Utc::now().format("%Y%m%dT%H%M%S%.3fZ").to_string()
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("sha256:{:x}", digest.finalize())
}

fn short_hash(value: &str) -> String {
    let hash = sha256(value.as_bytes());
    hash.trim_start_matches("sha256:")
        .chars()
        .take(16)
        .collect()
}

fn safe_slug(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= 96 {
            break;
        }
    }
    let out = out.trim_matches('_');
    if out.is_empty() {
        "value".to_string()
    } else {
        out.to_string()
    }
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

#[allow(dead_code)]
fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{}", std::process::id(), nanos)
}
