const views = {
  loading: document.getElementById('view-loading'),
  setup: document.getElementById('view-setup'),
  installing: document.getElementById('view-installing'),
  done: document.getElementById('view-done'),
};

function show(view) {
  Object.values(views).forEach((v) => v.classList.add('hidden'));
  views[view].classList.remove('hidden');
}

function setError(msg) {
  const box = document.getElementById('error-box');
  if (!msg) {
    box.classList.add('hidden');
    box.textContent = '';
    return;
  }
  box.textContent = msg;
  box.classList.remove('hidden');
}

function updateInstallButtonState() {
  const key = document.getElementById('apiKey').value.trim();
  const consent = document.getElementById('consent').checked;
  document.getElementById('installBtn').disabled = !(key && consent);
}

async function loadStatus() {
  const res = await fetch('/api/status');
  const status = await res.json();

  document.getElementById('d-claudedir').textContent = status.claudeDir;
  document.getElementById('d-user').textContent = `${status.username}@${status.hostname}`;
  document.getElementById('d-node').textContent = status.nodeVersion;

  const statusBadge = document.getElementById('d-status');
  if (status.alreadyInstalled) {
    statusBadge.textContent = 'Already installed (will refresh)';
    statusBadge.className = 'badge warn';
  } else {
    statusBadge.textContent = 'Ready to install';
    statusBadge.className = 'badge ok';
  }

  document.getElementById('apiUrl').placeholder = status.defaultApiUrl;

  show('setup');
}

document.getElementById('toggleKey').addEventListener('click', () => {
  const input = document.getElementById('apiKey');
  const btn = document.getElementById('toggleKey');
  if (input.type === 'password') {
    input.type = 'text';
    btn.textContent = 'Hide';
  } else {
    input.type = 'password';
    btn.textContent = 'Show';
  }
});

document.getElementById('apiKey').addEventListener('input', updateInstallButtonState);
document.getElementById('consent').addEventListener('change', updateInstallButtonState);

document.getElementById('installBtn').addEventListener('click', async () => {
  setError(null);
  const apiKey = document.getElementById('apiKey').value.trim();
  const apiUrl = document.getElementById('apiUrl').value.trim();

  show('installing');
  try {
    const res = await fetch('/api/install', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ apiKey, apiUrl }),
    });
    const result = await res.json();
    if (!result.ok) throw new Error(result.error || 'Install failed');

    const lines = [
      `Hooks:     ${result.tallyDir}\\hooks`,
      `Logs:      ${result.logsDir}`,
      `Settings:  ${result.settingsPath}`,
    ];
    if (result.backupPath) lines.push(`Backup:    ${result.backupPath}`);
    document.getElementById('done-paths').textContent = lines.join('\n');
    show('done');
    fetch('/api/shutdown', { method: 'POST' }).catch(() => {});
  } catch (err) {
    show('setup');
    setError(err.message);
  }
});

loadStatus().catch((err) => {
  show('setup');
  setError(`Could not detect environment: ${err.message}`);
});
