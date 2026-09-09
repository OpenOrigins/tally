use fs2::FileExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::Result;

const DEFAULT_SEGMENT_BYTES: u64 = 4 * 1024 * 1024;
const MIN_SEGMENT_BYTES: u64 = 64 * 1024;
const MAX_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
const SEGMENT_PREFIX: &str = "segment-";
const SEGMENT_SUFFIX: &str = ".jsonl";

#[derive(Clone, Debug)]
pub(crate) struct JournalRecord {
    pub sequence: u64,
    pub record_id: String,
    pub record: Value,
    pub log_root: Option<PathBuf>,
    pub private_paths: Vec<PathBuf>,
    pub private_objects: Vec<(PathBuf, Value)>,
}

pub(crate) fn append_record(
    state_dir: &Path,
    record: &Value,
    log_root: Option<&Path>,
    private_paths: &[PathBuf],
    private_objects: &[(PathBuf, Value)],
) -> Result<u64> {
    validate_record_integrity(record)?;
    let paths = JournalPaths::new(state_dir);
    super::create_private_dir(&paths.segments)?;
    let lock = super::open_private_lock(&paths.lock)?;
    lock.lock_exclusive()?;

    let result = (|| -> Result<u64> {
        let mut segments = segment_paths(&paths.segments)?;
        let checkpoint = read_checkpoint(&paths.checkpoint)?;
        let mut next_sequence = checkpoint.max(1);
        let mut active = segments.pop();

        if let Some((_, path)) = active.as_ref() {
            let last = repair_and_last_sequence(path)?;
            next_sequence = next_sequence.max(last.saturating_add(1));
        }

        let rotate = active
            .as_ref()
            .and_then(|(_, path)| fs::metadata(path).ok())
            .is_some_and(|metadata| metadata.len() >= configured_segment_bytes());
        if active.is_none() || rotate {
            let path = paths.segments.join(format!(
                "{SEGMENT_PREFIX}{next_sequence:020}{SEGMENT_SUFFIX}"
            ));
            active = Some((next_sequence, path));
        }
        let (_, active_path) = active.expect("active journal segment was selected");

        let mut stored_record = record.clone();
        stored_record["journal_sequence"] = Value::from(next_sequence);
        let record_id = stored_record["record_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("journal records require a non-empty record_id")?;
        let envelope = json!({
            "journal_version": 1,
            "sequence": next_sequence,
            "record_id": record_id,
            "record": stored_record,
            "local": {
                "log_root": log_root,
                "private_paths": private_paths,
                "private_objects": private_objects.iter().map(|(path, value)| json!({
                    "path": path,
                    "value": value,
                })).collect::<Vec<_>>(),
            },
        });
        append_json_line(&active_path, &envelope)?;
        Ok(next_sequence)
    })();

    FileExt::unlock(&lock)?;
    result
}

fn validate_record_integrity(record: &Value) -> Result<()> {
    const RECORD_TYPES: &[&str] = &[
        "SESSION_START",
        "INSTRUCTION_RECEIVED",
        "ACTION_TAKEN",
        "RESULT_RECEIVED",
        "HANDOFF",
        "TURN_END",
        "SESSION_END",
        "HEARTBEAT",
        "CODEX_LIFECYCLE",
        "CLAUDE_LIFECYCLE",
    ];
    let object = record
        .as_object()
        .ok_or("journal record must be a JSON object")?;
    let record_type = object["record_type"]
        .as_str()
        .ok_or("journal record is missing record_type")?;
    if !RECORD_TYPES.contains(&record_type) {
        return Err(format!("unsupported record_type {record_type}").into());
    }
    if object["schema_version"].as_str() != Some("0.2") {
        return Err("journal records must use schema_version 0.2".into());
    }

    let mut stack = vec![record];
    let mut visited = 0_usize;
    while let Some(value) = stack.pop() {
        visited += 1;
        if visited > 100_000 {
            return Err("journal record contains too many values".into());
        }
        match value {
            Value::Object(map) => {
                for (key, hash) in map {
                    if let Some(stem) = key.strip_suffix("_uri") {
                        let is_private_reference = hash
                            .as_str()
                            .is_some_and(|uri| uri.starts_with("private://sha256/"));
                        if is_private_reference && !map.contains_key(&format!("{stem}_hash")) {
                            return Err(format!(
                                "{key} is missing its matching {stem}_hash evidence field"
                            )
                            .into());
                        }
                    }
                    let Some(stem) = key.strip_suffix("_hash") else {
                        continue;
                    };
                    let Some(uri) = map.get(&format!("{stem}_uri")) else {
                        continue;
                    };
                    match (hash.as_str(), uri.as_str()) {
                        (Some(hash), Some(uri)) => validate_hash_uri_pair(hash, uri)?,
                        (None, None) if hash.is_null() && uri.is_null() => {}
                        (None, None) => {}
                        _ => {
                            return Err(format!(
                                "{key} and {stem}_uri must both be strings or both be null"
                            )
                            .into())
                        }
                    }
                }
                stack.extend(map.values());
            }
            Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    Ok(())
}

fn validate_hash_uri_pair(hash: &str, uri: &str) -> Result<()> {
    let Some(digest) = hash.strip_prefix("sha256:") else {
        return Err(format!("unsupported evidence hash {hash}").into());
    };
    let Some(uri_digest) = uri.strip_prefix("private://sha256/") else {
        return Err(format!("unsupported private evidence URI {uri}").into());
    };
    if digest != uri_digest {
        return Err(format!("evidence hash {hash} does not identify {uri}").into());
    }
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("evidence hash {hash} is not a SHA-256 digest").into());
    }
    Ok(())
}

pub(crate) fn pending_records(state_dir: &Path) -> Result<Vec<JournalRecord>> {
    let paths = JournalPaths::new(state_dir);
    super::create_private_dir(&paths.segments)?;
    let lock = super::open_private_lock(&paths.lock)?;
    lock.lock_exclusive()?;
    let result = pending_records_locked(state_dir, &paths);
    FileExt::unlock(&lock)?;
    result
}

fn pending_records_locked(state_dir: &Path, paths: &JournalPaths) -> Result<Vec<JournalRecord>> {
    let terminal = terminal_sequence(state_dir)?;
    let mut pending = Vec::new();
    let mut previous = 0_u64;

    let segments = segment_paths(&paths.segments)?;
    if let Some((_, active)) = segments.last() {
        repair_and_last_sequence(active)?;
    }
    for (_, path) in segments {
        for value in read_json_lines(&path)? {
            let record = decode_record(value)?;
            if record.sequence <= previous {
                return Err(format!(
                    "journal sequence {} is not greater than preceding sequence {previous}",
                    record.sequence
                )
                .into());
            }
            previous = record.sequence;
            if record.sequence > terminal {
                pending.push(record);
            }
        }
    }
    Ok(pending)
}

pub(crate) fn append_delivery_outcome(
    state_dir: &Path,
    record: &JournalRecord,
    status: &str,
    receipt: Option<&Value>,
    detail: Option<&str>,
) -> Result<()> {
    let paths = JournalPaths::new(state_dir);
    super::create_private_dir(&paths.root)?;
    let value = json!({
        "outcome_version": 1,
        "sequence": record.sequence,
        "record_id": record.record_id,
        "status": status,
        "receipt": receipt,
        "detail": detail,
        "recorded_at_unix_millis": super::agent_runtime::unix_now_millis(),
    });
    let previous = read_terminal_checkpoint(&paths.terminal_checkpoint)?;
    if record.sequence > previous.saturating_add(1) {
        return Err(format!(
            "delivery outcome sequence {} is not contiguous after {previous}",
            record.sequence
        )
        .into());
    }
    append_json_line_locked(&paths.outcomes, &paths.outcomes_lock, &value)?;
    super::atomic_write(
        &paths.terminal_checkpoint,
        record.sequence.max(previous).to_string().as_bytes(),
        0o600,
    )?;
    Ok(())
}

pub(crate) fn append_dead_letter(
    state_dir: &Path,
    record: &JournalRecord,
    detail: &str,
) -> Result<()> {
    let paths = JournalPaths::new(state_dir);
    super::create_private_dir(&paths.root)?;
    let value = json!({
        "dead_letter_version": 1,
        "sequence": record.sequence,
        "record_id": record.record_id,
        "detail": detail,
        "record": record.record,
        "recorded_at_unix_millis": super::agent_runtime::unix_now_millis(),
    });
    append_json_line_locked(&paths.dead_letters, &paths.dead_letters_lock, &value)
}

fn terminal_sequence(state_dir: &Path) -> Result<u64> {
    let paths = JournalPaths::new(state_dir);
    super::create_private_dir(&paths.root)?;
    let lock = super::open_private_lock(&paths.outcomes_lock)?;
    lock.lock_exclusive()?;
    let result = (|| -> Result<u64> {
        repair_torn_final_line(&paths.outcomes)?;
        read_terminal_checkpoint(&paths.terminal_checkpoint)
    })();
    FileExt::unlock(&lock)?;
    result
}

pub(crate) fn protected_private_paths(state_dir: &Path) -> Result<BTreeSet<PathBuf>> {
    let paths = JournalPaths::new(state_dir);
    super::create_private_dir(&paths.segments)?;
    let lock = super::open_private_lock(&paths.lock)?;
    lock.lock_exclusive()?;
    let result = (|| -> Result<BTreeSet<PathBuf>> {
        let terminal = terminal_sequence(state_dir)?;
        let mut protected = BTreeSet::new();
        let segments = segment_paths(&paths.segments)?;
        if let Some((_, active)) = segments.last() {
            repair_and_last_sequence(active)?;
        }
        for (_, path) in segments {
            for value in read_json_lines(&path)? {
                let record = decode_record(value)?;
                if record.sequence > terminal {
                    protected.extend(record.private_paths);
                }
            }
        }
        Ok(protected)
    })();
    FileExt::unlock(&lock)?;
    result
}

pub(crate) fn prune_completed_segments(state_dir: &Path) -> Result<u64> {
    let paths = JournalPaths::new(state_dir);
    let lock = super::open_private_lock(&paths.lock)?;
    lock.lock_exclusive()?;
    let result = (|| -> Result<u64> {
        let terminal = terminal_sequence(state_dir)?;
        let segments = segment_paths(&paths.segments)?;
        let active = segments.last().map(|(_, path)| path.clone());
        let mut removed = 0_u64;
        let mut checkpoint = read_checkpoint(&paths.checkpoint)?;
        for (_, path) in segments {
            if active.as_ref() == Some(&path) {
                continue;
            }
            let records = read_json_lines(&path)?
                .into_iter()
                .map(decode_record)
                .collect::<Result<Vec<_>>>()?;
            if records.iter().all(|record| record.sequence <= terminal) {
                checkpoint = checkpoint.max(
                    records
                        .last()
                        .map(|record| record.sequence.saturating_add(1))
                        .unwrap_or(checkpoint),
                );
                fs::remove_file(&path)?;
                super::sync_parent_directory(&path)?;
                removed += 1;
            }
        }
        if checkpoint > 0 {
            super::atomic_write(&paths.checkpoint, checkpoint.to_string().as_bytes(), 0o600)?;
        }
        Ok(removed)
    })();
    FileExt::unlock(&lock)?;
    result
}

fn decode_record(value: Value) -> Result<JournalRecord> {
    if value["journal_version"].as_u64() != Some(1) {
        return Err("unsupported journal record version".into());
    }
    let sequence = value["sequence"]
        .as_u64()
        .ok_or("journal record is missing sequence")?;
    let record_id = value["record_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("journal record is missing record_id")?
        .to_string();
    let record = value
        .get("record")
        .filter(|record| record.is_object())
        .ok_or("journal record is missing its structured record")?
        .clone();
    let log_root = value["local"]["log_root"].as_str().map(PathBuf::from);
    let private_paths = value["local"]["private_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .collect();
    let private_objects = value["local"]["private_objects"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|object| {
            let path = object["path"]
                .as_str()
                .map(PathBuf::from)
                .ok_or("journal private object is missing path")?;
            let value = object
                .get("value")
                .ok_or("journal private object is missing value")?
                .clone();
            Ok((path, value))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(JournalRecord {
        sequence,
        record_id,
        record,
        log_root,
        private_paths,
        private_objects,
    })
}

fn append_json_line_locked(path: &Path, lock_path: &Path, value: &Value) -> Result<()> {
    let lock = super::open_private_lock(lock_path)?;
    lock.lock_exclusive()?;
    let result = repair_torn_final_line(path).and_then(|()| append_json_line(path, value));
    FileExt::unlock(&lock)?;
    result
}

fn repair_torn_final_line(path: &Path) -> Result<()> {
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata()?.len();
    if length == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(());
    }

    let mut cursor = length;
    let mut buffer = vec![0_u8; 64 * 1024];
    let truncate_at = loop {
        let chunk = cursor.min(buffer.len() as u64) as usize;
        cursor -= chunk as u64;
        file.seek(SeekFrom::Start(cursor))?;
        file.read_exact(&mut buffer[..chunk])?;
        if let Some(index) = buffer[..chunk].iter().rposition(|byte| *byte == b'\n') {
            break cursor + index as u64 + 1;
        }
        if cursor == 0 {
            break 0;
        }
    };
    file.set_len(truncate_at)?;
    file.sync_all()?;
    Ok(())
}

fn append_json_line(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        super::create_private_dir(parent)?;
    }
    let existed = path.exists();
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    if !existed {
        super::sync_parent_directory(path)?;
    }
    Ok(())
}

fn read_json_lines(path: &Path) -> Result<Vec<Value>> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut values = Vec::new();
    for (index, line) in contents.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        values.push(serde_json::from_slice(line).map_err(|error| {
            format!(
                "could not decode {} line {}: {error}",
                path.display(),
                index + 1
            )
        })?);
    }
    Ok(values)
}

fn repair_and_last_sequence(path: &Path) -> Result<u64> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut length = file.metadata()?.len();
    if length == 0 {
        return Ok(0);
    }
    let mut window = 64 * 1024_u64;
    let mut tail;
    let mut start;
    loop {
        start = length.saturating_sub(window);
        file.seek(SeekFrom::Start(start))?;
        tail = Vec::with_capacity((length - start) as usize);
        file.read_to_end(&mut tail)?;
        let enough = start == 0 || tail.iter().filter(|byte| **byte == b'\n').count() >= 2;
        if enough {
            break;
        }
        window = window.saturating_mul(2);
    }
    if tail.last() != Some(&b'\n') {
        let valid_tail_len = tail
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        length = start.saturating_add(valid_tail_len as u64);
        file.set_len(length)?;
        file.sync_all()?;
        tail.truncate(valid_tail_len);
    }
    if length == 0 {
        return Ok(0);
    }
    let without_final_newline = &tail[..tail.len().saturating_sub(1)];
    let line_start = without_final_newline
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let value: Value = serde_json::from_slice(&without_final_newline[line_start..])?;
    let last = value["sequence"]
        .as_u64()
        .ok_or("journal record is missing sequence")?;
    file.seek(SeekFrom::End(0))?;
    Ok(last)
}

fn segment_paths(root: &Path) -> Result<Vec<(u64, PathBuf)>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(root)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let sequence = name
                .strip_prefix(SEGMENT_PREFIX)?
                .strip_suffix(SEGMENT_SUFFIX)?
                .parse::<u64>()
                .ok()?;
            path.is_file().then_some((sequence, path))
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|(sequence, _)| *sequence);
    Ok(paths)
}

fn read_checkpoint(path: &Path) -> Result<u64> {
    read_counter(path, 1)
}

fn read_terminal_checkpoint(path: &Path) -> Result<u64> {
    read_counter(path, 0)
}

fn read_counter(path: &Path, default: u64) -> Result<u64> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents.trim().parse::<u64>()?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(default),
        Err(error) => Err(error.into()),
    }
}

fn configured_segment_bytes() -> u64 {
    env::var("TALLY_JOURNAL_SEGMENT_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SEGMENT_BYTES)
        .clamp(MIN_SEGMENT_BYTES, MAX_SEGMENT_BYTES)
}

struct JournalPaths {
    root: PathBuf,
    segments: PathBuf,
    lock: PathBuf,
    outcomes: PathBuf,
    outcomes_lock: PathBuf,
    terminal_checkpoint: PathBuf,
    dead_letters: PathBuf,
    dead_letters_lock: PathBuf,
    checkpoint: PathBuf,
}

impl JournalPaths {
    fn new(state_dir: &Path) -> Self {
        let root = state_dir.join("journal");
        Self {
            segments: root.join("segments"),
            lock: root.join("journal.lock"),
            outcomes: root.join("delivery-outcomes.jsonl"),
            outcomes_lock: root.join("delivery-outcomes.lock"),
            terminal_checkpoint: root.join("terminal.checkpoint"),
            dead_letters: root.join("dead-letters.jsonl"),
            dead_letters_lock: root.join("dead-letters.lock"),
            checkpoint: root.join("sequence.checkpoint"),
            root,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_delivery_outcome, append_record, pending_records, prune_completed_segments,
    };
    use serde_json::json;
    use std::{env, fs};

    #[test]
    fn journal_preserves_order_and_prunes_only_closed_completed_segments() {
        let root = env::temp_dir().join(format!(
            "tally-journal-{}",
            crate::agent_runtime::unique_suffix()
        ));
        let private = root.join("private/object.json");
        let first =
            json!({"record_id": "one", "record_type": "SESSION_START", "schema_version": "0.2"});
        let second =
            json!({"record_id": "two", "record_type": "SESSION_END", "schema_version": "0.2"});
        assert_eq!(
            append_record(&root, &first, None, std::slice::from_ref(&private), &[],).unwrap(),
            1
        );
        assert_eq!(append_record(&root, &second, None, &[], &[]).unwrap(), 2);
        let pending = pending_records(&root).unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|record| record.record_id.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        append_delivery_outcome(&root, &pending[0], "delivered", None, None).unwrap();
        assert_eq!(pending_records(&root).unwrap().len(), 1);
        assert_eq!(prune_completed_segments(&root).unwrap(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journal_repairs_a_torn_final_line() {
        let root = env::temp_dir().join(format!(
            "tally-journal-torn-{}",
            crate::agent_runtime::unique_suffix()
        ));
        let record =
            json!({"record_id": "one", "record_type": "SESSION_START", "schema_version": "0.2"});
        append_record(&root, &record, None, &[], &[]).unwrap();
        let segment = fs::read_dir(root.join("journal/segments"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        use std::io::Write;
        fs::OpenOptions::new()
            .append(true)
            .open(segment)
            .unwrap()
            .write_all(b"{\"partial\":")
            .unwrap();
        let second =
            json!({"record_id": "two", "record_type": "SESSION_END", "schema_version": "0.2"});
        assert_eq!(append_record(&root, &second, None, &[], &[]).unwrap(), 2);
        assert_eq!(pending_records(&root).unwrap().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journal_rejects_mismatched_or_malformed_evidence_references() {
        let root = env::temp_dir().join(format!(
            "tally-journal-integrity-{}",
            crate::agent_runtime::unique_suffix()
        ));
        let digest = "a".repeat(64);
        let mismatch = json!({
            "record_id": "one",
            "record_type": "ACTION_TAKEN",
            "schema_version": "0.2",
            "params_hash": format!("sha256:{digest}"),
            "params_uri": format!("private://sha256/{}", "b".repeat(64)),
        });
        assert!(append_record(&root, &mismatch, None, &[], &[])
            .unwrap_err()
            .to_string()
            .contains("does not identify"));

        let malformed = json!({
            "record_id": "two",
            "record_type": "ACTION_TAKEN",
            "schema_version": "0.2",
            "params_hash": "sha256:not-a-digest",
            "params_uri": "private://sha256/not-a-digest",
        });
        assert!(append_record(&root, &malformed, None, &[], &[])
            .unwrap_err()
            .to_string()
            .contains("not a SHA-256 digest"));

        let orphaned = json!({
            "record_id": "three",
            "record_type": "ACTION_TAKEN",
            "schema_version": "0.2",
            "params_uri": format!("private://sha256/{digest}"),
        });
        assert!(append_record(&root, &orphaned, None, &[], &[])
            .unwrap_err()
            .to_string()
            .contains("missing its matching params_hash"));
        assert!(!root.exists());
    }

    #[test]
    fn journal_repairs_a_torn_delivery_outcome() {
        let root = env::temp_dir().join(format!(
            "tally-journal-outcome-{}",
            crate::agent_runtime::unique_suffix()
        ));
        let first =
            json!({"record_id": "one", "record_type": "SESSION_START", "schema_version": "0.2"});
        let second =
            json!({"record_id": "two", "record_type": "SESSION_END", "schema_version": "0.2"});
        append_record(&root, &first, None, &[], &[]).unwrap();
        append_record(&root, &second, None, &[], &[]).unwrap();
        let pending = pending_records(&root).unwrap();
        append_delivery_outcome(&root, &pending[0], "delivered", None, None).unwrap();
        let outcomes = root.join("journal/delivery-outcomes.jsonl");
        use std::io::Write;
        fs::OpenOptions::new()
            .append(true)
            .open(&outcomes)
            .unwrap()
            .write_all(b"{\"partial\":")
            .unwrap();

        let pending = pending_records(&root).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].record_id, "two");
        let contents = fs::read(&outcomes).unwrap();
        assert_eq!(contents.last(), Some(&b'\n'));
        assert!(!String::from_utf8(contents).unwrap().contains("partial"));
        fs::remove_dir_all(root).unwrap();
    }
}
