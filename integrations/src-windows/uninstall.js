#!/usr/bin/env node
// Reverses what server.js's doInstall() does: strips the anchor_hook.js
// command from ~/.claude/settings.json, stops any running heartbeat/forwarder
// daemons, and removes the installed hooks. Asks before deleting anything,
// and offers to keep or delete the collected log data separately.

const fs = require('fs');
const path = require('path');
const os = require('os');
const readline = require('readline');

function getClaudeDir() {
  if (process.env.CLAUDE_CONFIG_DIR) return process.env.CLAUDE_CONFIG_DIR;
  return path.join(os.homedir(), '.claude');
}

function readJsonSafe(file, fallback) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (_) {
    return fallback;
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

function ask(rl, question) {
  return new Promise((resolve) => rl.question(question, resolve));
}

async function main() {
  const claudeDir = getClaudeDir();
  const tallyDir = path.join(claudeDir, 'tally');
  const hooksDir = path.join(tallyDir, 'hooks');
  const logsDir = path.join(tallyDir, 'logs');
  const stateDir = path.join(logsDir, '.state');
  const settingsPath = path.join(claudeDir, 'settings.json');
  const installedRecord = readJsonSafe(path.join(stateDir, 'installed.json'), null);
  // Prefer the command exactly as the installer recorded it (robust across
  // different installer/uninstaller builds); fall back to recomputing it for
  // installs done before this record existed.
  const exeSuffix = process.platform === 'win32' ? '.exe' : '';
  const command = installedRecord ? installedRecord.command : `"${path.join(hooksDir, `anchor_hook${exeSuffix}`)}"`;

  console.log(`Tally Anchor uninstaller\nClaude config dir: ${claudeDir}\n`);

  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });

  const proceed = await ask(rl, 'Remove Tally Anchor hooks from this machine? [y/N] ');
  if (!/^y/i.test(proceed.trim())) {
    console.log('Cancelled.');
    rl.close();
    return;
  }

  // 1. Stop running daemons.
  for (const pidFile of ['forwarder.pid']) {
    const p = path.join(stateDir, pidFile);
    try {
      const pid = parseInt(fs.readFileSync(p, 'utf8'), 10);
      if (pid && isPidAlive(pid)) process.kill(pid);
      fs.unlinkSync(p);
    } catch (_) {}
  }
  try {
    for (const f of fs.readdirSync(stateDir)) {
      if (f.endsWith('.heartbeat.pid')) {
        const p = path.join(stateDir, f);
        try {
          const pid = parseInt(fs.readFileSync(p, 'utf8'), 10);
          if (pid && isPidAlive(pid)) process.kill(pid);
        } catch (_) {}
        try { fs.unlinkSync(p); } catch (_) {}
      }
    }
  } catch (_) {}

  // 2. Strip our hook entries from settings.json.
  const settings = readJsonSafe(settingsPath, null);
  if (settings && settings.hooks) {
    for (const event of Object.keys(settings.hooks)) {
      settings.hooks[event] = settings.hooks[event]
        .map((group) => ({
          ...group,
          hooks: (group.hooks || []).filter((h) => h.command !== command),
        }))
        .filter((group) => (group.hooks || []).length > 0);
      if (settings.hooks[event].length === 0) delete settings.hooks[event];
    }
    fs.writeFileSync(settingsPath, JSON.stringify(settings, null, 2));
    console.log(`Removed hook entries from ${settingsPath}`);
  }

  // 3. Remove the installed hook scripts and API key.
  try { fs.rmSync(hooksDir, { recursive: true, force: true }); } catch (_) {}
  try { fs.unlinkSync(path.join(stateDir, 'api_key.txt')); } catch (_) {}
  console.log(`Removed hook scripts and API key from ${tallyDir}`);

  const deleteLogs = await ask(rl, `Also delete collected log data at ${logsDir}? [y/N] `);
  if (/^y/i.test(deleteLogs.trim())) {
    try { fs.rmSync(logsDir, { recursive: true, force: true }); } catch (_) {}
    console.log('Log data deleted.');
  } else {
    console.log('Log data kept.');
  }

  rl.close();
  console.log('\nDone.');
}

main();
