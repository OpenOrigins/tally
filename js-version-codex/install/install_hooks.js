#!/usr/bin/env node
// Installs this project's Codex hooks into the real, global Codex hooks
// config. Unlike Claude Code (which auto-discovers a project-local
// .claude/settings.json), Codex reads hooks from a single file at
// $CODEX_HOME/hooks.json (default ~/.codex/hooks.json) — there is no
// per-project scoping — so getting logging wired up means merging our
// entries into that file rather than just committing one in-repo.
//
// This script is idempotent: it removes any handlers it previously
// installed (matched by this project's own absolute path appearing in the
// command string) before re-adding the current set, so re-running it after
// moving/renaming this checkout doesn't leave stale duplicate entries.
//
// Run: node install/install_hooks.js  (or `npm run install-hooks`)

const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

const PROJECT_DIR = path.resolve(__dirname, '..');
const PROJECT_DIR_POSIX = PROJECT_DIR.split(path.sep).join('/');
const TEMPLATE_PATH = path.join(PROJECT_DIR, '.codex', 'hooks.json.template');

const CODEX_HOME = process.env.CODEX_HOME || path.join(os.homedir(), '.codex');
const HOOKS_PATH = process.env.CODEX_HOOKS_PATH || path.join(CODEX_HOME, 'hooks.json');

// On Windows, a bare "bash" on PATH resolves to C:\Windows\System32\bash.exe
// — the WSL launcher stub — before it resolves to Git Bash, whether or not
// WSL has a distro installed. Codex spawns hook commands directly (not
// inside whatever shell the user's terminal happens to be), so it hits
// that stub and the hook silently fails with no distro configured. Resolve
// Git Bash's real executable up front and bake its absolute path into
// hooks.json instead of relying on "bash" + PATH order at hook-run time.
function resolveBash() {
  if (process.env.CODEX_HOOK_BASH) return process.env.CODEX_HOOK_BASH;
  if (process.platform !== 'win32') return 'bash';

  // Must be the top-level <gitroot>\bin\bash.exe shim, not
  // <gitroot>\usr\bin\bash.exe (the raw MSYS2 binary): the shim sets up
  // MSYSTEM/PATH so mkdir/date/dirname/cat resolve, the raw binary run
  // standalone has none of that and every coreutils call in the script
  // fails with "command not found".
  const candidates = [];

  try {
    const gitOut = execFileSync('where', ['git'], { encoding: 'utf8' });
    const gitExe = gitOut.split(/\r?\n/).map((l) => l.trim()).find(Boolean);
    if (gitExe) {
      // "where git" may return cmd\git.exe, mingw64\bin\git.exe, or bin\git.exe
      // depending on install layout — walk up ancestors looking for the
      // install root's bin\bash.exe rather than assuming a fixed depth.
      let dir = path.dirname(gitExe);
      for (let i = 0; i < 4 && dir; i++) {
        candidates.push(path.join(dir, 'bin', 'bash.exe'));
        const parent = path.dirname(dir);
        if (parent === dir) break;
        dir = parent;
      }
    }
  } catch (_) {}

  candidates.push('C:\\Program Files\\Git\\bin\\bash.exe');
  candidates.push('C:\\Program Files (x86)\\Git\\bin\\bash.exe');

  try {
    const whereOut = execFileSync('where', ['bash'], { encoding: 'utf8' });
    for (const line of whereOut.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (trimmed) candidates.push(trimmed);
    }
  } catch (_) {}

  const found = candidates.find((c) => {
    const normalized = c.toLowerCase();
    if (normalized.includes('system32') || normalized.includes('windowsapps')) return false;
    // reject the raw MSYS2 binary under usr\bin — only the bin\ shim works standalone
    if (/[\\/]usr[\\/]bin[\\/]bash\.exe$/.test(normalized)) return false;
    try {
      return fs.existsSync(c);
    } catch (_) {
      return false;
    }
  });

  if (!found) {
    throw new Error(
      'Could not find a real Git Bash executable (not the WSL System32\\bash.exe stub). ' +
      'Install Git for Windows, or set CODEX_HOOK_BASH to the full path of bash.exe.'
    );
  }
  return found;
}

const BASH_PATH = resolveBash();
// Quote here (not in the template) since the resolved path may contain
// spaces (e.g. "C:\Program Files\Git\bin\bash.exe"). Quotes are JSON-escaped
// (\") to match how the template itself quotes {{PROJECT_DIR}}, since this
// substitution happens on the raw JSON text before it's parsed.
const BASH_COMMAND = `\\"${BASH_PATH.split(path.sep).join('/')}\\"`;

function loadTemplate() {
  const raw = fs
    .readFileSync(TEMPLATE_PATH, 'utf8')
    .split('{{PROJECT_DIR}}').join(PROJECT_DIR_POSIX)
    .split('{{BASH}}').join(BASH_COMMAND);
  return JSON.parse(raw);
}

function loadExisting(hooksPath) {
  if (!fs.existsSync(hooksPath)) return { hooks: {} };
  try {
    const parsed = JSON.parse(fs.readFileSync(hooksPath, 'utf8'));
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      if (!parsed.hooks || typeof parsed.hooks !== 'object') parsed.hooks = {};
      return parsed;
    }
  } catch (_) {}
  throw new Error(`Refusing to modify ${hooksPath}: not a JSON object with a "hooks" field`);
}

function isOurHandler(handler) {
  const command = handler && handler.command;
  return typeof command === 'string' && command.includes(PROJECT_DIR_POSIX);
}

function removeOurHandlers(config) {
  let removed = 0;
  for (const event of Object.keys(config.hooks)) {
    const groups = config.hooks[event];
    if (!Array.isArray(groups)) continue;
    config.hooks[event] = groups
      .map((group) => {
        if (!group || !Array.isArray(group.hooks)) return group;
        const before = group.hooks.length;
        group.hooks = group.hooks.filter((h) => !isOurHandler(h));
        removed += before - group.hooks.length;
        return group;
      })
      .filter((group) => group && Array.isArray(group.hooks) && group.hooks.length > 0);
    if (config.hooks[event].length === 0) delete config.hooks[event];
  }
  return removed;
}

function backup(hooksPath) {
  if (!fs.existsSync(hooksPath)) return null;
  const stamp = new Date().toISOString().replace(/[:.]/g, '-');
  const backupPath = `${hooksPath}.backup-${stamp}`;
  fs.copyFileSync(hooksPath, backupPath);
  return backupPath;
}

function main() {
  const template = loadTemplate();
  const config = loadExisting(HOOKS_PATH);
  const removed = removeOurHandlers(config);

  for (const [event, groups] of Object.entries(template.hooks)) {
    if (!Array.isArray(config.hooks[event])) config.hooks[event] = [];
    config.hooks[event].push(...groups);
  }

  fs.mkdirSync(path.dirname(HOOKS_PATH), { recursive: true });
  const backupPath = backup(HOOKS_PATH);
  fs.writeFileSync(HOOKS_PATH, JSON.stringify(config, null, 2) + '\n');

  console.log(`Installed Tally Codex hooks into ${HOOKS_PATH}`);
  console.log(`Using bash: ${BASH_PATH}`);
  if (removed > 0) console.log(`Replaced ${removed} previously-installed handler(s) from this project.`);
  if (backupPath) console.log(`Backed up previous file to ${backupPath}`);
  console.log('');
  console.log('Codex also needs hooks enabled in its config.toml. Make sure this file:');
  console.log(`  ${path.join(CODEX_HOME, 'config.toml')}`);
  console.log('contains:');
  console.log('  [features]');
  console.log('  hooks = true');
  console.log('');
  console.log(`Logs will be written under: ${path.join(PROJECT_DIR, 'logs')}`);
}

main();
