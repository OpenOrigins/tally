#!/usr/bin/env node
// Ships new logs/anchor_log.jsonl records to the configured Tally ingest API
// as they're appended.
//
// Singleton background process: anchor_hook.js spawns this on SessionStart
// (see startForwarderDaemon()) if no instance is already running, tracked via
// logs/.state/forwarder.pid. Unlike heartbeat_daemon.js it is NOT per-session
// -- one instance drains the shared log for every session, and keeps running
// (up to MAX_RUNTIME_MS) even between sessions so it can catch up on
// anything queued while the API was unreachable.
//
// Delivery model: tails the log from a persisted byte offset
// (logs/.state/forwarder_offset.txt) and POSTs each record in file order,
// one at a time. The offset only advances after a POST succeeds, so a crash
// or API outage just pauses shipping -- it never skips a record, and at most
// re-sends the one record that was in flight when it died (at-least-once,
// not exactly-once). If no offset file exists yet (first run), the offset is
// initialized to the log's current end, so pre-existing backlog is skipped
// and only records appended from that point on are ever forwarded.
//
// The API key and endpoint URL are both read fresh from disk on every drain
// rather than cached at startup, so rotating logs/.state/api_key.txt or
// editing logs/.state/config.json takes effect without restarting the daemon.

const fs = require('fs');
const path = require('path');
const https = require('https');
const http = require('http');

// Compiled with pkg into a standalone binary -- __dirname is a virtual
// snapshot path at build time, not this binary's real location on disk.
const REAL_DIR = process.pkg ? path.dirname(process.execPath) : __dirname;
const TALLY_ROOT = path.resolve(REAL_DIR, '..');
const LOG_DIR = path.join(TALLY_ROOT, 'logs');
const LOG_FILE = path.join(LOG_DIR, 'anchor_log.jsonl');
const STATE_DIR = path.join(LOG_DIR, '.state');
const OFFSET_FILE = path.join(STATE_DIR, 'forwarder_offset.txt');
const PID_FILE = path.join(STATE_DIR, 'forwarder.pid');
const ERROR_LOG = path.join(STATE_DIR, 'forwarder_errors.log');
const API_KEY_FILE = path.join(STATE_DIR, 'api_key.txt');
const CONFIG_FILE = path.join(STATE_DIR, 'config.json');

const DEFAULT_API_URL = 'https://api.dev2.openorigins.com/v1/tally/logs';
const INGEST_SOURCE = 'sdk';
const INGEST_PATH = 'anchor-log-forwarder';

const POLL_MS = 5000; // fs.watch fallback -- catches events missed/coalesced by the OS
const MAX_RUNTIME_MS = 24 * 60 * 60 * 1000; // safety net; next SessionStart respawns it
const MAX_BACKOFF_MS = 60 * 1000;

fs.mkdirSync(STATE_DIR, { recursive: true });

function readApiKey() {
  try {
    return fs.readFileSync(API_KEY_FILE, 'utf8').trim();
  } catch (_) {
    return null;
  }
}

function readApiUrl() {
  try {
    const cfg = JSON.parse(fs.readFileSync(CONFIG_FILE, 'utf8'));
    if (cfg && typeof cfg.apiUrl === 'string' && cfg.apiUrl) return cfg.apiUrl;
  } catch (_) {}
  return DEFAULT_API_URL;
}

function readOffset() {
  try {
    return parseInt(fs.readFileSync(OFFSET_FILE, 'utf8'), 10) || 0;
  } catch (_) {
    // No persisted offset yet: skip any backlog already in the log file and
    // start from the current end, so only newly-appended records get shipped.
    // Persist this baseline immediately -- otherwise every future call would
    // hit this same fallback and re-adopt "now" as the start, never advancing.
    let offset = 0;
    try {
      offset = fs.statSync(LOG_FILE).size;
    } catch (_) {
      offset = 0;
    }
    writeOffset(offset);
    return offset;
  }
}

function writeOffset(offset) {
  fs.writeFileSync(OFFSET_FILE, String(offset));
}

function logError(message) {
  try {
    fs.appendFileSync(ERROR_LOG, `[${new Date().toISOString()}] ${message}\n`);
  } catch (_) {
    // best-effort only
  }
}

function postRecord(record, apiKey, apiUrl) {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify(record);
    const url = new URL(apiUrl);
    const transport = url.protocol === 'http:' ? http : https;
    const req = transport.request(
      {
        hostname: url.hostname,
        port: url.port || (url.protocol === 'http:' ? 80 : 443),
        path: url.pathname + url.search,
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(body),
          'x-api-key': apiKey,
          'x-oo-tally-source': INGEST_SOURCE,
          'x-oo-tally-ingest-path': INGEST_PATH,
        },
        timeout: 15000,
      },
      (res) => {
        let responseBody = '';
        res.on('data', (chunk) => { responseBody += chunk; });
        res.on('end', () => {
          if (res.statusCode >= 200 && res.statusCode < 300) {
            resolve();
          } else {
            reject(new Error(`HTTP ${res.statusCode}: ${responseBody.slice(0, 300)}`));
          }
        });
      },
    );
    req.on('timeout', () => req.destroy(new Error('request timed out')));
    req.on('error', reject);
    req.write(body);
    req.end();
  });
}

async function sendWithRetry(record, apiKey, apiUrl) {
  let attempt = 0;
  for (;;) {
    try {
      await postRecord(record, apiKey, apiUrl);
      return;
    } catch (err) {
      attempt += 1;
      logError(`send failed (attempt ${attempt}) for ${record.record_type || 'unknown'}/${record.session_id || 'n/a'}: ${err.message}`);
      const backoff = Math.min(MAX_BACKOFF_MS, 1000 * 2 ** (attempt - 1));
      await new Promise((r) => setTimeout(r, backoff));
    }
  }
}

function isPidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (_) {
    return false;
  }
}

// Atomically claims PID_FILE via exclusive create so at most one forwarder
// ever runs, even if anchor_hook.js's SessionStart handler spawns several in
// quick succession (its own alive-check is TOCTOU-racy). Without this, two
// live forwarders would both tail the same offset file and each send the
// records in between -- duplicate POSTs to the API.
function acquireSingletonLock(pidFile) {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      const fd = fs.openSync(pidFile, 'wx');
      fs.writeSync(fd, String(process.pid));
      fs.closeSync(fd);
      return true;
    } catch (err) {
      if (err.code !== 'EEXIST') throw err;
      let existingPid;
      try {
        existingPid = parseInt(fs.readFileSync(pidFile, 'utf8'), 10);
      } catch (_) {
        continue; // pid file vanished between our open() and read(); retry the create
      }
      if (existingPid && isPidAlive(existingPid)) return false; // genuinely already running
      try { fs.unlinkSync(pidFile); } catch (_) {} // stale lock left by a crashed instance
    }
  }
  return false; // couldn't resolve the lock after retries; don't risk a duplicate instance
}

let draining = false;
let redrainRequested = false;

async function drainOnce() {
  const apiKey = readApiKey();
  if (!apiKey) {
    logError(`no API key found at ${API_KEY_FILE}; skipping drain`);
    return;
  }
  const apiUrl = readApiUrl();

  let offset = readOffset();
  for (;;) {
    let stat;
    try {
      stat = fs.statSync(LOG_FILE);
    } catch (_) {
      return; // log file doesn't exist yet
    }
    if (stat.size < offset) {
      // File is smaller than our persisted offset -- it was truncated or
      // recreated (rotation, manual clear, etc). Whatever was written between
      // the old offset and now is already gone/unsendable, so snap forward to
      // the current end and resume live tailing from there.
      logError(`log file shrank below persisted offset (offset=${offset}, size=${stat.size}); resetting offset to ${stat.size}`);
      offset = stat.size;
      writeOffset(offset);
    }
    if (stat.size <= offset) return; // nothing new

    const buf = Buffer.alloc(stat.size - offset);
    const fd = fs.openSync(LOG_FILE, 'r');
    fs.readSync(fd, buf, 0, buf.length, offset);
    fs.closeSync(fd);

    const text = buf.toString('utf8');
    const lastNewline = text.lastIndexOf('\n');
    if (lastNewline === -1) return; // only a partial line buffered so far -- wait for the rest

    const lines = text.slice(0, lastNewline).split('\n').filter((l) => l.length > 0);

    for (const line of lines) {
      let record;
      try {
        record = JSON.parse(line);
      } catch (err) {
        logError(`skipping malformed line: ${err.message}`);
        offset += Buffer.byteLength(line, 'utf8') + 1;
        writeOffset(offset);
        continue;
      }
      await sendWithRetry(record, apiKey, apiUrl);
      offset += Buffer.byteLength(line, 'utf8') + 1;
      writeOffset(offset);
    }
  }
}

// Coalesces overlapping triggers (fs.watch callback, poll timer) into a single
// drain loop, then immediately re-runs once if a new trigger arrived mid-drain.
async function drain() {
  if (draining) {
    redrainRequested = true;
    return;
  }
  draining = true;
  try {
    do {
      redrainRequested = false;
      await drainOnce();
    } while (redrainRequested);
  } finally {
    draining = false;
  }
}

const startedAt = Date.now();
if (!acquireSingletonLock(PID_FILE)) {
  // Another forwarder already holds the lock -- exit quietly rather than
  // risk double-sending anything.
  process.exit(0);
}

function cleanupAndExit() {
  try { fs.unlinkSync(PID_FILE); } catch (_) {}
  process.exit(0);
}

drain();

try {
  // Watch the directory rather than the file so this survives the file not
  // existing yet at startup (fs.watch on a missing path throws immediately).
  fs.watch(LOG_DIR, { persistent: true }, (_event, filename) => {
    if (filename === path.basename(LOG_FILE)) drain();
  });
} catch (err) {
  logError(`fs.watch unavailable, relying on ${POLL_MS}ms poll only: ${err.message}`);
}

const pollTimer = setInterval(() => {
  if (Date.now() - startedAt > MAX_RUNTIME_MS) {
    clearInterval(pollTimer);
    cleanupAndExit();
    return;
  }
  drain();
}, POLL_MS);

process.on('SIGTERM', cleanupAndExit);
process.on('SIGINT', cleanupAndExit);
