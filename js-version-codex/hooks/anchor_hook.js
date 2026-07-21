#!/usr/bin/env node
// Anchor-style audit log for real Codex CLI sessions in this project.
//
// Every wired hook event (see hooks.json, installed via
// install/install_hooks.js) pipes its JSON payload into this script via
// stdin. It re-shapes that payload into a record that mirrors the
// OpenOrigins Anchor log schema (SESSION_START, HEARTBEAT,
// INSTRUCTION_RECEIVED, ACTION_TAKEN, RESULT_RECEIVED, HANDOFF, SESSION_END)
// and appends exactly one JSON line to logs/anchor_log.jsonl (a single,
// append-only, one-record-per-line file, same file for every session), and
// inserts the same record as a row into logs/anchor_log.sqlite for queryable
// storage.
//
// This mirrors js-version/hooks/anchor_hook.js (the Claude Code version) as
// closely as the two CLIs' hook shapes allow. The main difference: Codex has
// no SessionEnd event, so the session-close record fires on "Stop" instead,
// and session ids are read from whichever field Codex actually sends
// (session_id, thread_id, or conversation_id).
//
// This is a local, unsigned log: no real cryptography, no external anchor
// service. pre_state_hash/post_state_hash are fixed placeholders (not
// computed from real state) — deliberately, so the schema shape matches
// without pretending to verify anything it doesn't. anchor_receipt values
// are locally-generated sequence ids, not receipts from a real anchor.
//
// Runs as a pure observer: always exits 0 with no stdout, so it can never
// block a tool call or influence a permission decision, same guarantee as
// hooks/log_hook.sh which is wired alongside it.

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { spawn } = require('child_process');
const Database = require('better-sqlite3');

// Resolved from this script's own location, not the caller's cwd, so the log
// always lands in this project's logs/ dir no matter which directory Codex
// was launched from. CODEX_PROJECT_DIR is an escape hatch, not something the
// Codex CLI is known to set itself (unlike Claude Code's $CLAUDE_PROJECT_DIR).
const PROJECT_DIR = process.env.CODEX_PROJECT_DIR || path.resolve(__dirname, '..');
const LOG_DIR = path.join(PROJECT_DIR, 'logs');
const LOG_FILE = path.join(LOG_DIR, 'anchor_log.jsonl');
const DB_FILE = path.join(LOG_DIR, 'anchor_log.sqlite');
const STATE_DIR = path.join(LOG_DIR, '.anchor_state');
const HEARTBEAT_SCRIPT = path.join(__dirname, 'heartbeat_daemon.js');

const FIXED_PRE_STATE_HASH = 'sha256:' + '0'.repeat(64) + ' (placeholder, not computed)';
const FIXED_POST_STATE_HASH = 'sha256:' + '1'.repeat(64) + ' (placeholder, not computed)';

fs.mkdirSync(LOG_DIR, { recursive: true });
fs.mkdirSync(STATE_DIR, { recursive: true });

const db = new Database(DB_FILE);
db.pragma('journal_mode = WAL');
db.exec(`
  CREATE TABLE IF NOT EXISTS anchor_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    record_type TEXT NOT NULL,
    session_id TEXT,
    anchor_receipt TEXT,
    recorded_at TEXT NOT NULL,
    payload TEXT NOT NULL
  );
  CREATE INDEX IF NOT EXISTS idx_anchor_log_session ON anchor_log (session_id);
  CREATE INDEX IF NOT EXISTS idx_anchor_log_record_type ON anchor_log (record_type);
`);
const insertRecordStmt = db.prepare(
  'INSERT INTO anchor_log (record_type, session_id, anchor_receipt, recorded_at, payload) VALUES (@record_type, @session_id, @anchor_receipt, @recorded_at, @payload)'
);

function nowIso() {
  return new Date().toISOString().replace(/\.\d+Z$/, 'Z');
}

function shortId() {
  return crypto.randomBytes(4).toString('hex');
}

function nextReceipt() {
  const seqFile = path.join(STATE_DIR, 'seq.txt');
  let seq = 0;
  try { seq = parseInt(fs.readFileSync(seqFile, 'utf8'), 10) || 0; } catch (_) {}
  seq += 1;
  fs.writeFileSync(seqFile, String(seq));
  return `rcpt_local_${String(seq).padStart(6, '0')}_${shortId()}`;
}

function statePath(sessionId) {
  return path.join(STATE_DIR, `${sessionId}.json`);
}

function loadState(sessionId) {
  try {
    return JSON.parse(fs.readFileSync(statePath(sessionId), 'utf8'));
  } catch (_) {
    return { lastInstructionId: null };
  }
}

function saveState(sessionId, state) {
  fs.writeFileSync(statePath(sessionId), JSON.stringify(state));
}

function appendRecord(record) {
  fs.appendFileSync(LOG_FILE, JSON.stringify(record) + '\n');
  insertRecordStmt.run({
    record_type: record.record_type,
    session_id: record.session_id || null,
    anchor_receipt: record.anchor_receipt || null,
    recorded_at: nowIso(),
    payload: JSON.stringify(record),
  });
}

function truncate(value, max = 500) {
  const s = typeof value === 'string' ? value : JSON.stringify(value);
  if (s === undefined) return null;
  return s.length > max ? s.slice(0, max) + `...(${s.length - max} more chars truncated)` : s;
}

// Codex's hook payload shapes aren't as strictly documented as Claude Code's,
// so fields are looked up defensively across the aliases each event has been
// observed to use, instead of assuming one exact key.
function firstDefined(obj, keys) {
  for (const key of keys) {
    if (obj && obj[key] !== undefined && obj[key] !== null && obj[key] !== '') return obj[key];
  }
  return undefined;
}

function extractSessionId(payload) {
  return firstDefined(payload, ['session_id', 'thread_id', 'conversation_id', 'conversationId']) || 'unknown';
}

function heartbeatPidFile(sessionId) {
  return path.join(STATE_DIR, `${sessionId}.heartbeat.pid`);
}

function isPidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (_) {
    return false;
  }
}

function startHeartbeatDaemon(sessionId) {
  const pidFile = heartbeatPidFile(sessionId);
  try {
    const existingPid = parseInt(fs.readFileSync(pidFile, 'utf8'), 10);
    if (existingPid && isPidAlive(existingPid)) return; // already running for this session
  } catch (_) {}

  const child = spawn(process.execPath, [HEARTBEAT_SCRIPT, sessionId, LOG_FILE, DB_FILE], {
    detached: true,
    stdio: 'ignore',
    windowsHide: true,
  });
  fs.writeFileSync(pidFile, String(child.pid));
  child.unref();
}

function stopHeartbeatDaemon(sessionId) {
  const pidFile = heartbeatPidFile(sessionId);
  try {
    const pid = parseInt(fs.readFileSync(pidFile, 'utf8'), 10);
    if (pid && isPidAlive(pid)) process.kill(pid);
  } catch (_) {}
  try { fs.unlinkSync(pidFile); } catch (_) {}
}

let raw = '';
process.stdin.on('data', (chunk) => { raw += chunk; });
process.stdin.on('end', () => {
  try {
    // Strip a leading UTF-8 BOM: some Windows process-spawning paths write
    // it ahead of the JSON payload, which would otherwise fail JSON.parse
    // and get silently swallowed below, looking identical to "hook never
    // fired" from the log's perspective.
    const stripped = raw.charCodeAt(0) === 0xfeff ? raw.slice(1) : raw;
    main(JSON.parse(stripped), process.argv[2]);
  } catch (_) {
    // Malformed/empty stdin: stay a silent observer, never fail the real hook.
  }
  try { db.close(); } catch (_) {}
  process.exit(0);
});

function main(payload, argvEvent) {
  const event = payload.hook_event_name || argvEvent || 'Unknown';
  const sessionId = `sess_${extractSessionId(payload)}`;
  const ts = nowIso();

  switch (event) {
    case 'SessionStart': {
      appendRecord({
        record_type: 'SESSION_START',
        session_id: sessionId,
        agent_id: 'local-agent:codex',
        agent_version: payload.model || payload.codex_version || 'unknown',
        principal: { type: 'user', id: 'user:shuhood@openorigins.com' },
        source: payload.source || 'unknown',
        session_started_at: ts,
        anchor_receipt: nextReceipt(),
      });
      startHeartbeatDaemon(sessionId);
      break;
    }

    case 'UserPromptSubmit': {
      const instructionId = `instr_${shortId()}`;
      const state = loadState(sessionId);
      state.lastInstructionId = instructionId;
      saveState(sessionId, state);

      const prompt = firstDefined(payload, ['prompt', 'user_prompt', 'input', 'text', 'content']);
      appendRecord({
        record_type: 'INSTRUCTION_RECEIVED',
        session_id: sessionId,
        instruction_id: instructionId,
        sender: { id: 'user:shuhood@openorigins.com' },
        declared_intent: { summary: truncate(prompt, 500) },
        instruction_received_at: ts,
        anchor_receipt: nextReceipt(),
      });
      break;
    }

    case 'PreToolUse': {
      const state = loadState(sessionId);
      const toolName = firstDefined(payload, ['tool_name', 'toolName', 'name', 'command', 'recipient_name']) || 'unknown';
      const toolParams = firstDefined(payload, ['tool_input', 'arguments', 'args', 'params', 'input']);
      appendRecord({
        record_type: 'ACTION_TAKEN',
        session_id: sessionId,
        action_id: `act_${payload.tool_use_id || payload.call_id || shortId()}`,
        instruction_id: state.lastInstructionId,
        action_type: 'tool_call',
        tool: {
          name: toolName,
          params: truncate(toolParams, 500),
        },
        pre_state_hash: FIXED_PRE_STATE_HASH,
        post_state_hash: FIXED_POST_STATE_HASH,
        action_timestamp: ts,
        anchor_receipt: nextReceipt(),
        deviance_flag: { deviated: false, delta_category: null },
      });
      break;
    }

    case 'PostToolUse': {
      const result = firstDefined(payload, ['tool_response', 'result', 'output']);
      appendRecord({
        record_type: 'RESULT_RECEIVED',
        session_id: sessionId,
        action_id: `act_${payload.tool_use_id || payload.call_id || shortId()}`,
        result_interpretation: { summary: truncate(result, 500) },
        result_received_at: ts,
        exception: { occurred: Boolean(payload.error || payload.exception) },
        anchor_receipt: nextReceipt(),
      });
      break;
    }

    case 'SubagentStart': {
      appendRecord({
        record_type: 'HANDOFF',
        session_id: sessionId,
        handoff_id: `hoff_${shortId()}`,
        emitting_party: 'sender',
        sender: { agent_id: 'local-agent:codex' },
        receiver: { agent_id: `local-agent:subagent:${payload.agent_type || 'unknown'}` },
        handoff_timestamp: ts,
        acknowledgement_status: 'pending',
        anchor_receipt: nextReceipt(),
      });
      break;
    }

    // Codex has no SessionEnd event; a turn/session closes via "Stop".
    case 'Stop': {
      appendRecord({
        record_type: 'SESSION_END',
        session_id: sessionId,
        outcome: payload.reason || 'codex_turn_stopped',
        session_ended_at: ts,
        anchor_receipt: nextReceipt(),
      });
      stopHeartbeatDaemon(sessionId);
      try { fs.unlinkSync(statePath(sessionId)); } catch (_) {}
      break;
    }

    default:
      // Other wired events (PermissionRequest, PreCompact, PostCompact,
      // SubagentStop) don't map cleanly onto the Anchor schema; they're
      // still captured raw by hooks/log_hook.sh alongside this script.
      break;
  }
}
