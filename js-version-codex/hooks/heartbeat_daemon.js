#!/usr/bin/env node
// Background heartbeat for the Anchor-style log (see hooks/anchor_hook.js).
//
// Spawned detached by anchor_hook.js's SessionStart handler, one per Codex
// session_id. Appends a HEARTBEAT record to the shared anchor_log.jsonl
// and anchor_log.sqlite every INTERVAL_MS while the session is open, so any
// gap between real actions still shows periodic liveness records instead of
// silence.
//
// Codex has no SessionEnd event and its "Stop" hook's reliability across
// every termination path (Ctrl+C, window close, kill) isn't documented, so
// this daemon also self-terminates after MAX_RUNTIME_MS regardless, to avoid
// leaking an orphaned background process if the clean-shutdown hook never
// fires. This file is unchanged in working from js-version/hooks/heartbeat_daemon.js
// — the daemon itself doesn't know or care which CLI is driving it.

const fs = require('fs');
const Database = require('better-sqlite3');

const [, , sessionId, logFile, dbFile] = process.argv;
if (!sessionId || !logFile || !dbFile) process.exit(1);

const INTERVAL_MS = 60 * 1000;
const MAX_RUNTIME_MS = 6 * 60 * 60 * 1000; // 6h safety net
const startedAt = Date.now();

const db = new Database(dbFile);
db.pragma('journal_mode = WAL');
const insertHeartbeatStmt = db.prepare(
  'INSERT INTO anchor_log (record_type, session_id, anchor_receipt, recorded_at, payload) VALUES (@record_type, @session_id, NULL, @recorded_at, @payload)'
);

function appendHeartbeat() {
  const record = {
    record_type: 'HEARTBEAT',
    session_id: sessionId,
    timestamp: new Date().toISOString().replace(/\.\d+Z$/, 'Z'),
  };
  try {
    fs.appendFileSync(logFile, JSON.stringify(record) + '\n');
    insertHeartbeatStmt.run({
      record_type: record.record_type,
      session_id: record.session_id,
      recorded_at: record.timestamp,
      payload: JSON.stringify(record),
    });
  } catch (_) {
    // Log file/dir gone: nothing sensible left to do.
    try { db.close(); } catch (_) {}
    process.exit(0);
  }
}

const timer = setInterval(() => {
  if (Date.now() - startedAt > MAX_RUNTIME_MS) {
    clearInterval(timer);
    try { db.close(); } catch (_) {}
    process.exit(0);
    return;
  }
  appendHeartbeat();
}, INTERVAL_MS);
