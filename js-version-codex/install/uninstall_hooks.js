#!/usr/bin/env node
// Removes this project's handlers from the global Codex hooks.json (see
// install_hooks.js for why this has to be a global-file merge/unmerge
// instead of a project-local settings file the way Claude Code's is).
//
// Run: node install/uninstall_hooks.js  (or `npm run uninstall-hooks`)

const fs = require('fs');
const os = require('os');
const path = require('path');

const PROJECT_DIR = path.resolve(__dirname, '..');
const PROJECT_DIR_POSIX = PROJECT_DIR.split(path.sep).join('/');

const CODEX_HOME = process.env.CODEX_HOME || path.join(os.homedir(), '.codex');
const HOOKS_PATH = process.env.CODEX_HOOKS_PATH || path.join(CODEX_HOME, 'hooks.json');

function isOurHandler(handler) {
  const command = handler && handler.command;
  return typeof command === 'string' && command.includes(PROJECT_DIR_POSIX);
}

function main() {
  if (!fs.existsSync(HOOKS_PATH)) {
    console.log(`No hooks file found at ${HOOKS_PATH}`);
    return;
  }
  const config = JSON.parse(fs.readFileSync(HOOKS_PATH, 'utf8'));
  if (!config || typeof config !== 'object' || typeof config.hooks !== 'object') {
    throw new Error(`Refusing to modify ${HOOKS_PATH}: unexpected shape`);
  }

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

  const stamp = new Date().toISOString().replace(/[:.]/g, '-');
  const backupPath = `${HOOKS_PATH}.backup-${stamp}`;
  fs.copyFileSync(HOOKS_PATH, backupPath);
  fs.writeFileSync(HOOKS_PATH, JSON.stringify(config, null, 2) + '\n');

  console.log(`Removed ${removed} Tally hook handler(s) from ${HOOKS_PATH}`);
  console.log(`Backed up previous file to ${backupPath}`);
}

main();
