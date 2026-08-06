#!/usr/bin/env node
// Local installer backend for the Tally Anchor hooks.
//
// Serves a small GUI on 127.0.0.1 (never bound to a public interface) that
// walks the user through installing hooks/anchor_hook.js,
// hooks/heartbeat_daemon.js and hooks/log_forwarder.js into their GLOBAL
// Claude Code config directory (so it applies to every project on this
// machine), wiring them into ~/.claude/settings.json, and storing the API
// key they type in locally on their own disk.
//
// Nothing about who the user is or which machine this is gets baked into
// the installer package itself -- username/hostname/client surface are all
// detected fresh by anchor_hook.js at runtime, on the installing machine.

const http = require('http');
const https = require('https');
const { URL } = require('url');
const fs = require('fs');
const path = require('path');
const os = require('os');
const crypto = require('crypto');
const { exec } = require('child_process');

// When compiled with pkg into a standalone binary, __dirname is a virtual
// snapshot path baked in at build time, not this file's real location on
// disk -- and pkg's asset-embedding has proven unreliable across platforms
// in testing. So this native build ships gui/ and bin/ as plain files sitting
// next to the executable, and we resolve against process.execPath's real
// directory instead of __dirname whenever running as a compiled binary.
const INSTALLER_DIR = process.pkg ? path.dirname(process.execPath) : __dirname;
const GUI_DIR = path.join(INSTALLER_DIR, 'gui');
const SOURCE_BIN_DIR = path.join(INSTALLER_DIR, 'bin');

const DEFAULT_API_URL = 'https://api.dev2.openorigins.com/v1/tally/logs';
const HOOK_EVENTS = ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'SubagentStart', 'SessionEnd'];

const EXE_SUFFIX = process.platform === 'win32' ? '.exe' : '';
const HOOK_BIN_FILES = {
  anchor: `anchor_hook${EXE_SUFFIX}`,
  heartbeat: `heartbeat_daemon${EXE_SUFFIX}`,
  forwarder: `log_forwarder${EXE_SUFFIX}`,
};

function getClaudeDir() {
  if (process.env.CLAUDE_CONFIG_DIR) return process.env.CLAUDE_CONFIG_DIR;
  return path.join(os.homedir(), '.claude');
}

function getTallyDir(claudeDir) {
  return path.join(claudeDir, 'tally');
}

function anchorHookCommand(tallyDir) {
  const hookPath = path.join(tallyDir, 'hooks', HOOK_BIN_FILES.anchor);
  // The hook itself is a standalone compiled binary -- no node prefix needed.
  return `"${hookPath}"`;
}

function readJsonSafe(file, fallback) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (_) {
    return fallback;
  }
}

function detectStatus() {
  const claudeDir = getClaudeDir();
  const settingsPath = path.join(claudeDir, 'settings.json');
  const tallyDir = getTallyDir(claudeDir);
  const apiKeyPath = path.join(tallyDir, 'logs', '.state', 'api_key.txt');

  const settings = readJsonSafe(settingsPath, null);
  const command = anchorHookCommand(tallyDir);
  let alreadyWired = false;
  if (settings && settings.hooks) {
    const sessionStartHooks = settings.hooks.SessionStart || [];
    alreadyWired = sessionStartHooks.some((group) =>
      (group.hooks || []).some((h) => h.command === command),
    );
  }

  let username = 'unknown';
  try { username = os.userInfo().username || 'unknown'; } catch (_) {}

  return {
    claudeDir,
    settingsPath,
    claudeDirExists: fs.existsSync(claudeDir),
    settingsFound: !!settings,
    tallyDir,
    alreadyInstalled: alreadyWired,
    hasApiKey: fs.existsSync(apiKeyPath),
    nodeVersion: process.version,
    platform: process.platform,
    username,
    hostname: os.hostname(),
    defaultApiUrl: DEFAULT_API_URL,
  };
}

// Let the server know a client just finished wiring up the hooks.
// Best-effort only -- a failure here must never block or fail the local
// install -- but the caller does await this (with a timeout) so the process
// doesn't exit via /api/shutdown before the request has a chance to land.
function notifyClientConnected(apiKey, apiUrl) {
  return new Promise((resolve) => {
    let settled = false;
    const done = () => {
      if (settled) return;
      settled = true;
      resolve();
    };

    try {
      const target = new URL('https://api.dev2.openorigins.com/v1/tally/onboarding/client-connected');
      const payload = JSON.stringify({ source: 'claude-code' });
      const req = https.request(
        {
          hostname: target.hostname,
          port: target.port || 443,
          path: target.pathname,
          method: 'POST',
          headers: {
            'x-api-key': apiKey,
            'Content-Type': 'application/json',
            'Content-Length': Buffer.byteLength(payload),
          },
          timeout: 5000,
        },
        (res) => {
          res.resume();
          res.on('end', done);
        },
      );
      req.on('error', (err) => {
        console.error('client-connected notification failed:', err.message);
        done();
      });
      req.on('timeout', () => {
        req.destroy(new Error('timed out'));
      });
      req.write(payload);
      req.end();
    } catch (err) {
      console.error('client-connected notification failed:', err.message);
      done();
    }
  });
}

function backupFile(file) {
  if (!fs.existsSync(file)) return null;
  const stamp = new Date().toISOString().replace(/[:.]/g, '-');
  const backupPath = `${file}.bak-${stamp}`;
  fs.copyFileSync(file, backupPath);
  return backupPath;
}

function mergeHooksIntoSettings(settings, tallyDir) {
  const command = anchorHookCommand(tallyDir);
  const merged = { ...settings };
  merged.hooks = { ...(merged.hooks || {}) };

  for (const event of HOOK_EVENTS) {
    const existingGroups = merged.hooks[event] || [];
    const alreadyPresent = existingGroups.some((group) =>
      (group.hooks || []).some((h) => h.command === command),
    );
    if (alreadyPresent) {
      merged.hooks[event] = existingGroups;
      continue;
    }
    merged.hooks[event] = [
      ...existingGroups,
      { hooks: [{ type: 'command', command }] },
    ];
  }
  return merged;
}

async function doInstall({ apiKey, apiUrl }) {
  if (!apiKey || !apiKey.trim()) {
    throw new Error('API key is required.');
  }

  const claudeDir = getClaudeDir();
  const tallyDir = getTallyDir(claudeDir);
  const hooksDestDir = path.join(tallyDir, 'hooks');
  const logsDir = path.join(tallyDir, 'logs');
  const stateDir = path.join(logsDir, '.state');
  const settingsPath = path.join(claudeDir, 'settings.json');

  fs.mkdirSync(claudeDir, { recursive: true });
  fs.mkdirSync(hooksDestDir, { recursive: true });
  fs.mkdirSync(stateDir, { recursive: true });

  // 1. Copy the three compiled hook binaries (self-contained -- no Node.js
  // needed on this machine for them to run).
  for (const file of Object.values(HOOK_BIN_FILES)) {
    const dest = path.join(hooksDestDir, file);
    fs.copyFileSync(path.join(SOURCE_BIN_DIR, file), dest);
    try { fs.chmodSync(dest, 0o755); } catch (_) {}
  }

  // 2. Write the API key locally (0600 where supported). Never transmitted
  // anywhere by the installer itself -- only read later by log_forwarder.js
  // over HTTPS to the configured endpoint.
  const apiKeyPath = path.join(stateDir, 'api_key.txt');
  fs.writeFileSync(apiKeyPath, apiKey.trim(), { mode: 0o600 });
  try { fs.chmodSync(apiKeyPath, 0o600); } catch (_) {}

  // 3. Write forwarder config (endpoint URL only -- no identity data).
  const configPath = path.join(stateDir, 'config.json');
  fs.writeFileSync(configPath, JSON.stringify({ apiUrl: apiUrl && apiUrl.trim() ? apiUrl.trim() : DEFAULT_API_URL }, null, 2));

  // 4. Merge hook wiring into the user's global settings.json, backing up
  // whatever was there first so this is easy to reverse.
  const existingSettings = readJsonSafe(settingsPath, {});
  const backupPath = backupFile(settingsPath);
  const mergedSettings = mergeHooksIntoSettings(existingSettings, tallyDir);
  fs.writeFileSync(settingsPath, JSON.stringify(mergedSettings, null, 2));

  // 5. Record exactly what was installed so the uninstaller can find and
  // remove the right settings.json entries without having to recompute the
  // command string itself (which could drift if uninstall runs from a
  // different binary/build than install did).
  fs.writeFileSync(
    path.join(stateDir, 'installed.json'),
    JSON.stringify({ command: anchorHookCommand(tallyDir), settingsPath, hooksDestDir }, null, 2),
  );

  // 6. Tell the server the hooks are wired up, using the same key the user
  // just entered. Best-effort -- doesn't fail the install -- but we await it
  // so the GUI's immediate follow-up /api/shutdown call can't kill the
  // process before this request has finished.
  await notifyClientConnected(apiKey.trim(), apiUrl);

  return {
    claudeDir,
    tallyDir,
    settingsPath,
    backupPath,
    logsDir,
  };
}

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
};

function serveStatic(req, res) {
  let reqPath = req.url === '/' ? '/index.html' : req.url;
  reqPath = reqPath.split('?')[0];
  const filePath = path.join(GUI_DIR, reqPath);
  if (!filePath.startsWith(GUI_DIR)) {
    res.writeHead(403);
    res.end('Forbidden');
    return;
  }
  fs.readFile(filePath, (err, data) => {
    if (err) {
      res.writeHead(404);
      res.end('Not found');
      return;
    }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(filePath)] || 'application/octet-stream' });
    res.end(data);
  });
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    let data = '';
    req.on('data', (chunk) => { data += chunk; });
    req.on('end', () => resolve(data));
    req.on('error', reject);
  });
}

const server = http.createServer(async (req, res) => {
  try {
    if (req.method === 'GET' && req.url === '/api/status') {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify(detectStatus()));
      return;
    }

    if (req.method === 'POST' && req.url === '/api/install') {
      const body = JSON.parse((await readBody(req)) || '{}');
      const result = await doInstall(body);
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ ok: true, ...result }));
      return;
    }

    if (req.method === 'POST' && req.url === '/api/shutdown') {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ ok: true }));
      setTimeout(() => process.exit(0), 200);
      return;
    }

    serveStatic(req, res);
  } catch (err) {
    res.writeHead(500, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ ok: false, error: err.message }));
  }
});

const PORT = 0; // let the OS pick a free port
server.listen(PORT, '127.0.0.1', () => {
  const { port } = server.address();
  const url = `http://127.0.0.1:${port}/`;
  console.log(`Tally installer running at ${url}`);

  const platform = process.platform;
  const openCmd = platform === 'win32' ? `start "" "${url}"` : platform === 'darwin' ? `open "${url}"` : `xdg-open "${url}"`;
  exec(openCmd, () => {});
});

// Safety net: never listen on anything but loopback.
server.on('connection', (socket) => {
  if (socket.remoteAddress !== '127.0.0.1' && socket.remoteAddress !== '::1' && socket.remoteAddress !== '::ffff:127.0.0.1') {
    socket.destroy();
  }
});
