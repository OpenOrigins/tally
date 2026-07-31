#!/usr/bin/env node
// Anchor-style audit log for Claude Code sessions, installed globally
// (~/.claude/tally/hooks/anchor_hook.js) so it fires for every project on
// this machine, not just one repo.
//
// Every wired hook event (see the "hooks" block merged into the user's
// global ~/.claude/settings.json by the installer) pipes its JSON payload
// into this script via stdin. It re-shapes that payload into a record that
// mirrors the OpenOrigins Anchor log schema (SESSION_START, HEARTBEAT,
// INSTRUCTION_RECEIVED, ACTION_TAKEN, RESULT_RECEIVED, HANDOFF, SESSION_END)
// and appends one JSON line per record to logs/anchor_log.jsonl.
//
// Identity (who ran this) and client (which Claude Code surface: CLI,
// editor, desktop) are both detected locally at runtime from this machine's
// OS/environment -- never baked in at install time and never shipped as
// part of the installer itself.
//
// This is a local, unsigned log: no real cryptography, no external anchor
// service. pre_state_hash/post_state_hash are fixed placeholders (not
// computed from real state) -- deliberately, so the schema shape matches
// without pretending to verify anything it doesn't.
//
// Runs as a pure observer: always exits 0 with no stdout, so it can never
// block a tool call or influence a permission decision.

const fs = require('fs');
const path = require('path');
const os = require('os');
const crypto = require('crypto');
const { spawn } = require('child_process');

// This file is compiled with pkg into a standalone anchor_hook(.exe) binary.
// Under pkg, __dirname is a virtual snapshot path baked in at build time, not
// this binary's real location on disk -- process.execPath (the actual running
// executable) is the reliable one for finding sibling files/writing logs.
const REAL_DIR = process.pkg ? path.dirname(process.execPath) : __dirname;
const EXE_SUFFIX = process.platform === 'win32' ? '.exe' : '';

const TALLY_ROOT = path.resolve(REAL_DIR, '..');
const LOG_DIR = path.join(TALLY_ROOT, 'logs');
const LOG_FILE = path.join(LOG_DIR, 'anchor_log.jsonl');
const STATE_DIR = path.join(LOG_DIR, '.state');
// Sibling heartbeat/forwarder binaries, installed alongside this one.
const HEARTBEAT_BIN = path.join(REAL_DIR, `heartbeat_daemon${EXE_SUFFIX}`);
const FORWARDER_BIN = path.join(REAL_DIR, `log_forwarder${EXE_SUFFIX}`);

const FIXED_PRE_STATE_HASH = 'sha256:' + '0'.repeat(64) + ' (placeholder, not computed)';
const FIXED_POST_STATE_HASH = 'sha256:' + '1'.repeat(64) + ' (placeholder, not computed)';

fs.mkdirSync(LOG_DIR, { recursive: true });
fs.mkdirSync(STATE_DIR, { recursive: true });

function nowIso() {
  return new Date().toISOString().replace(/\.\d+Z$/, 'Z');
}

function shortId() {
  return crypto.randomBytes(4).toString('hex');
}

// --- Identity: detected locally, every time, never hardcoded. ---
function detectPrincipalId() {
  let username = 'unknown';
  try { username = os.userInfo().username || 'unknown'; } catch (_) {}
  let host = 'unknown-host';
  try { host = os.hostname() || host; } catch (_) {}
  return `user:${username}@${host}`;
}

// --- Client surface: best-effort heuristic from env vars available to the
// hook process. Claude Code doesn't expose a documented "which surface am I"
// signal, so this is inferred, not guaranteed -- treat as advisory. ---
function detectClientSurface() {
  const env = process.env;
  if (env.CURSOR_TRACE_ID) return 'editor:cursor';
  if (env.TERM_PROGRAM === 'vscode' || env.VSCODE_PID || env.VSCODE_GIT_IPC_HANDLE) return 'editor:vscode';
  if (env.TERMINAL_EMULATOR === 'JetBrains-JediTerm' || env.JETBRAINS_IDE) return 'editor:jetbrains';
  if (env.TERM_PROGRAM) return `terminal:${env.TERM_PROGRAM}`;
  if (process.platform === 'win32' && env.WT_SESSION) return 'terminal:windows-terminal';
  return 'terminal:unknown';
}

function statePath(sessionId) {
  return path.join(STATE_DIR, `${sessionId}.json`);
}

function loadState(sessionId) {
  try {
    return JSON.parse(fs.readFileSync(statePath(sessionId), 'utf8'));
  } catch (_) {
    return { lastInstructionId: null, agentId: null };
  }
}

function saveState(sessionId, state) {
  fs.writeFileSync(statePath(sessionId), JSON.stringify(state));
}

function appendRecord(record) {
  fs.appendFileSync(LOG_FILE, JSON.stringify(record) + '\n');
}

function truncate(value, max = 500) {
  const s = typeof value === 'string' ? value : JSON.stringify(value);
  if (s === undefined) return null;
  return s.length > max ? s.slice(0, max) + `...(${s.length - max} more chars truncated)` : s;
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

function startHeartbeatDaemon(sessionId, agentId) {
  const pidFile = heartbeatPidFile(sessionId);
  try {
    const existingPid = parseInt(fs.readFileSync(pidFile, 'utf8'), 10);
    if (existingPid && isPidAlive(existingPid)) return; // already running for this session
  } catch (_) {}

  const child = spawn(HEARTBEAT_BIN, [sessionId, LOG_FILE, agentId], {
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

function forwarderPidFile() {
  return path.join(STATE_DIR, 'forwarder.pid');
}

// One forwarder instance ships the whole shared log, independent of any
// single session -- it's started opportunistically on SessionStart but never
// stopped on SessionEnd, so it keeps draining/retrying between sessions too.
function startForwarderDaemon() {
  const pidFile = forwarderPidFile();
  try {
    const existingPid = parseInt(fs.readFileSync(pidFile, 'utf8'), 10);
    if (existingPid && isPidAlive(existingPid)) return; // already running
  } catch (_) {}

  const child = spawn(FORWARDER_BIN, [], {
    detached: true,
    stdio: 'ignore',
    windowsHide: true,
  });
  fs.writeFileSync(pidFile, String(child.pid));
  child.unref();
}

let raw = '';
process.stdin.on('data', (chunk) => { raw += chunk; });
process.stdin.on('end', () => {
  try {
    main(JSON.parse(raw));
  } catch (_) {
    // Malformed/empty stdin: stay a silent observer, never fail the real hook.
  }
  process.exit(0);
});

function main(payload) {
  const event = payload.hook_event_name || 'Unknown';
  const sessionId = `sess_${payload.session_id || 'unknown'}`;
  const ts = nowIso();
  const principalId = detectPrincipalId();
  const clientSurface = detectClientSurface();

  switch (event) {
    case 'SessionStart': {
      const agentId = `local-agent:claude-code:${payload.model || 'unknown'}`;
      const state = loadState(sessionId);
      state.agentId = agentId;
      saveState(sessionId, state);

      appendRecord({
        schema_version: '0.2',
        record_type: 'SESSION_START',
        session_id: sessionId,
        agent_id: agentId,
        agent_version: payload.model || 'unknown',
        client_surface: clientSurface,
        principal: { type: 'user', id: principalId },
        source: payload.source || 'unknown',
        session_started_at: ts,
      });
      startHeartbeatDaemon(sessionId, agentId);
      startForwarderDaemon();
      break;
    }

    case 'UserPromptSubmit': {
      const instructionId = `instr_${shortId()}`;
      const state = loadState(sessionId);
      state.lastInstructionId = instructionId;
      saveState(sessionId, state);

      appendRecord({
        schema_version: '0.2',
        record_type: 'INSTRUCTION_RECEIVED',
        session_id: sessionId,
        instruction_id: instructionId,
        sender: { id: principalId },
        declared_intent: { summary: truncate(payload.prompt, 500) },
        instruction_received_at: ts,
      });
      break;
    }

    case 'PreToolUse': {
      const state = loadState(sessionId);
      appendRecord({
        schema_version: '0.2',
        record_type: 'ACTION_TAKEN',
        session_id: sessionId,
        action_id: `act_${payload.tool_use_id || shortId()}`,
        instruction_id: state.lastInstructionId,
        action_type: 'tool_call',
        tool: {
          name: payload.tool_name || 'unknown',
          params: truncate(payload.tool_input, 500),
        },
        pre_state_hash: FIXED_PRE_STATE_HASH,
        post_state_hash: FIXED_POST_STATE_HASH,
        action_timestamp: ts,
        deviance_flag: { deviated: false, delta_category: null },
      });
      break;
    }

    case 'PostToolUse': {
      appendRecord({
        schema_version: '0.2',
        record_type: 'RESULT_RECEIVED',
        session_id: sessionId,
        action_id: `act_${payload.tool_use_id || shortId()}`,
        result_interpretation: { summary: truncate(payload.tool_response, 500) },
        result_received_at: ts,
        exception: { occurred: false },
      });
      break;
    }

    case 'SubagentStart': {
      const state = loadState(sessionId);
      appendRecord({
        schema_version: '0.2',
        record_type: 'HANDOFF',
        session_id: sessionId,
        handoff_id: `hoff_${shortId()}`,
        emitting_party: 'sender',
        sender: { agent_id: state.agentId || 'local-agent:claude-code' },
        receiver: { agent_id: `local-agent:subagent:${payload.agent_type || 'unknown'}` },
        handoff_timestamp: ts,
        acknowledgement_status: 'pending',
      });
      break;
    }

    case 'SessionEnd': {
      appendRecord({
        schema_version: '0.2',
        record_type: 'SESSION_END',
        session_id: sessionId,
        outcome: payload.reason || 'unknown',
        session_ended_at: ts,
      });
      stopHeartbeatDaemon(sessionId);
      try { fs.unlinkSync(statePath(sessionId)); } catch (_) {}
      break;
    }

    default:
      break;
  }
}
