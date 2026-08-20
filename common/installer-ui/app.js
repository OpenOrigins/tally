const sections = ["setup", "remove", "installing", "done"];
const tokenFromHash = new URLSearchParams(window.location.hash.slice(1)).get("token");
if (tokenFromHash) {
  sessionStorage.setItem("tallyInstallerToken", tokenFromHash);
  history.replaceState(null, "", window.location.pathname);
}
const token = sessionStorage.getItem("tallyInstallerToken") || "";
let clients = [];
let pendingRemovalClients = [];

function show(name) {
  sections.forEach((id) => { document.getElementById(id).hidden = id !== name; });
}

async function api(path, body = {}) {
  const response = await fetch(path, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-tally-installer-token": token,
    },
    body: JSON.stringify(body),
  });
  const result = await response.json();
  if (!response.ok || !result.ok) throw new Error(result.error || "Installer request failed");
  return result;
}

async function load() {
  if (!token) throw new Error("This installer session has expired. Reopen the Tally installer.");
  const status = await api("/api/status");
  clients = status.clients;
  document.title = "Tally Installer";
  document.getElementById("title").textContent = "Connect Tally";
  document.getElementById("clientChoices").replaceChildren(...clients.map(clientChoice));
  document.getElementById("configPaths").replaceChildren(...clients.map(configPathField));
  document.getElementById("apiUrl").value = status.defaultApiUrl;
  const installed = clients.some((client) => client.installed);
  document.getElementById("status").textContent = installed ? "Update" : "Setup";
  document.getElementById("uninstallButton").hidden = !installed;
  show("setup");
}

function clientChoice(client) {
  const label = document.createElement("label");
  label.className = "client-choice";
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = true;
  input.dataset.client = client.id;
  const text = document.createElement("span");
  text.textContent = client.product;
  const state = document.createElement("small");
  state.textContent = client.installed ? "Installed" : "Ready";
  label.append(input, text, state);
  return label;
}

function configPathField(client) {
  const wrapper = document.createElement("div");
  wrapper.className = "config-field";
  const label = document.createElement("label");
  label.htmlFor = `config-${client.id}`;
  label.textContent = `${client.product} configuration path`;
  const input = document.createElement("input");
  input.id = `config-${client.id}`;
  input.type = "text";
  input.spellcheck = false;
  input.value = client.configPath;
  wrapper.append(label, input);
  return wrapper;
}

function selectedClients() {
  return clients.filter((client) => document.querySelector(`[data-client="${client.id}"]`).checked)
    .map((client) => ({
      id: client.id,
      configPath: document.getElementById(`config-${client.id}`).value,
    }));
}

function showResultDetails(resultClients, removal = null) {
  const names = resultClients.map((result) => clients.find((client) => client.id === result.id).product);
  document.getElementById("doneClients").textContent = names.join(", ");
  document.getElementById("doneConfig").textContent = resultClients.map((result) => result.configPath).join("; ");
  document.getElementById("doneKey").textContent = resultClients.map((result) => result.keyPath).filter(Boolean).join("; ") || "Removed";
  const dataRow = document.getElementById("doneDataRow");
  dataRow.hidden = removal === null;
  if (removal !== null) {
    const paths = [...new Set(resultClients.flatMap((result) => [result.queuePath, result.logsPath]).filter(Boolean))];
    document.getElementById("doneData").textContent = removal
      ? "Deleted"
      : `Retained at ${paths.join("; ")}`;
  }
}

document.getElementById("showKey").addEventListener("change", (event) => {
  document.getElementById("apiKey").type = event.target.checked ? "text" : "password";
});

document.getElementById("installForm").addEventListener("submit", async (event) => {
  event.preventDefault();
  const apiKeyInput = document.getElementById("apiKey");
  const button = document.getElementById("installButton");
  const error = document.getElementById("error");
  const warning = document.getElementById("warning");
  const retryButton = document.getElementById("retryButton");
  error.hidden = true;
  warning.hidden = true;
  retryButton.hidden = true;
  document.getElementById("resultMark").classList.remove("warning-mark");
  button.disabled = true;
  document.getElementById("progressTitle").textContent = "Installing Tally";
  document.getElementById("progressMessage").textContent = "Writing local credentials and updating hooks.";
  show("installing");
  try {
    const selected = selectedClients();
    if (selected.length === 0) throw new Error("Choose Codex, Claude Code, or both.");
    const result = await api("/api/install", {
      apiKey: apiKeyInput.value,
      apiUrl: document.getElementById("apiUrl").value,
      clients: selected,
    });
    apiKeyInput.value = "";
    showResultDetails(result.clients);
    if (result.connected) {
      sessionStorage.removeItem("tallyInstallerToken");
      document.getElementById("doneTitle").textContent = "Tally is connected";
      document.getElementById("doneMessage").textContent = "Hooks are installed and the OpenOrigins dashboard confirmed every selected client.";
    } else {
      document.getElementById("resultMark").classList.add("warning-mark");
      document.getElementById("doneTitle").textContent = "Tally is installed locally";
      document.getElementById("doneMessage").textContent = "Local logging will continue, but the dashboard could not confirm this client automatically.";
      warning.textContent = result.warning;
      warning.hidden = false;
      retryButton.hidden = false;
      button.disabled = false;
    }
    document.getElementById("status").textContent = result.connected ? "Connected" : "Installed with warning";
    show("done");
  } catch (requestError) {
    error.textContent = requestError.message;
    error.hidden = false;
    button.disabled = false;
    show("setup");
  }
});

document.getElementById("uninstallButton").addEventListener("click", () => {
  const error = document.getElementById("error");
  error.hidden = true;
  pendingRemovalClients = selectedClients();
  if (pendingRemovalClients.length === 0) {
    error.textContent = "Choose Codex, Claude Code, or both.";
    error.hidden = false;
    return;
  }
  const names = pendingRemovalClients.map((selection) => clients.find((client) => client.id === selection.id).product);
  document.getElementById("removeClients").textContent = names.join(", ");
  document.getElementById("removeData").checked = false;
  document.getElementById("confirmUninstallButton").textContent = "Remove integrations";
  document.getElementById("removeError").hidden = true;
  document.getElementById("status").textContent = "Remove";
  show("remove");
});

document.getElementById("removeData").addEventListener("change", (event) => {
  document.getElementById("confirmUninstallButton").textContent = event.target.checked
    ? "Remove integrations and data"
    : "Remove integrations";
});

document.getElementById("confirmUninstallButton").addEventListener("click", async () => {
  const button = document.getElementById("uninstallButton");
  const confirmButton = document.getElementById("confirmUninstallButton");
  const installButton = document.getElementById("installButton");
  const error = document.getElementById("removeError");
  const removeData = document.getElementById("removeData").checked;
  error.hidden = true;
  button.disabled = true;
  confirmButton.disabled = true;
  installButton.disabled = true;
  document.getElementById("resultMark").classList.remove("warning-mark");
  document.getElementById("progressTitle").textContent = "Removing Tally";
  document.getElementById("progressMessage").textContent = removeData
    ? "Removing hooks, credentials, queued records, and local logs."
    : "Removing hooks and local credentials while retaining queued records and logs.";
  document.getElementById("status").textContent = "Removing";
  show("installing");
  try {
    const result = await api("/api/uninstall", {
      clients: pendingRemovalClients,
      removeData,
    });
    sessionStorage.removeItem("tallyInstallerToken");
    document.getElementById("doneTitle").textContent = result.dataRemoved
      ? "Tally integrations and data are removed"
      : "Tally integrations are removed";
    document.getElementById("doneMessage").textContent = result.dataRemoved
      ? "Hooks, local credentials, installed hook helpers, queued records, and local logs were removed."
      : "Hooks, local credentials, and installed hook helpers were removed. Queued records and local logs were retained.";
    showResultDetails(result.clients, result.dataRemoved);
    document.getElementById("warning").hidden = true;
    document.getElementById("retryButton").hidden = true;
    document.querySelector(".close-note").textContent = "The installer file remains. Close this window, then delete it or uninstall the Homebrew cask if you no longer need it.";
    document.getElementById("status").textContent = "Removed";
    show("done");
  } catch (requestError) {
    error.textContent = requestError.message;
    error.hidden = false;
    button.disabled = false;
    confirmButton.disabled = false;
    installButton.disabled = false;
    document.getElementById("status").textContent = "Remove";
    show("remove");
  }
});

document.getElementById("backButton").addEventListener("click", () => {
  pendingRemovalClients = [];
  document.getElementById("status").textContent = "Update";
  show("setup");
});

document.getElementById("retryButton").addEventListener("click", () => {
  document.getElementById("status").textContent = "Retry";
  document.getElementById("error").hidden = true;
  show("setup");
  document.getElementById("apiKey").focus();
});

function shutdown() {
  api("/api/shutdown").catch(() => {});
  window.close();
}

document.getElementById("cancelButton").addEventListener("click", shutdown);
document.getElementById("doneCloseButton").addEventListener("click", shutdown);

load().catch((loadError) => {
  document.getElementById("status").textContent = "Unavailable";
  const error = document.getElementById("error");
  error.textContent = loadError.message;
  error.hidden = false;
  show("setup");
});
