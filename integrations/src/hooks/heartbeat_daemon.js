#!/usr/bin/env node
// Background heartbeat for the Anchor-style log (see hooks/anchor_hook.js).
//
// Spawned detached by anchor_hook.js's SessionStart handler, one per Claude
// Code session_id. Appends a HEARTBEAT record to the shared anchor_log.jsonl
// every INTERVAL_MS while the session is open, so any gap between real
// actions still shows periodic liveness records instead of silence.
//
// SessionEnd's reliability across every termination path (Ctrl+C, window
// close, kill) isn't documented, so this daemon also self-terminates after
// MAX_RUNTIME_MS regardless, to avoid leaking an orphaned background
// process if the clean-shutdown hook never fires.

const fs = require('fs');

const [, , sessionId, logFile, agentId] = process.argv;
if (!sessionId || !logFile) process.exit(1);

const INTERVAL_MS = 60 * 1000;
const MAX_RUNTIME_MS = 6 * 60 * 60 * 1000; // 6h safety net
const startedAt = Date.now();

function appendHeartbeat() {
  const record = {
    schema_version: '0.2',
    record_type: 'HEARTBEAT',
    session_id: sessionId,
    agent_id: agentId || 'local-agent:claude-code:unknown',
    timestamp: new Date().toISOString().replace(/\.\d+Z$/, 'Z'),
  };
  try {
    fs.appendFileSync(logFile, JSON.stringify(record) + '\n');
  } catch (_) {
    // Log file/dir gone: nothing sensible left to do.
    process.exit(0);
  }
}

const timer = setInterval(() => {
  if (Date.now() - startedAt > MAX_RUNTIME_MS) {
    clearInterval(timer);
    process.exit(0);
    return;
  }
  appendHeartbeat();
}, INTERVAL_MS);
