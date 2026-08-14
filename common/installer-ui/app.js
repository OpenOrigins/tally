const sections = ["setup", "installing", "done"];
const tokenFromHash = new URLSearchParams(window.location.hash.slice(1)).get("token");
if (tokenFromHash) {
  sessionStorage.setItem("tallyInstallerToken", tokenFromHash);
  history.replaceState(null, "", window.location.pathname);
}
const token = sessionStorage.getItem("tallyInstallerToken") || "";
let clients = [];

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

function showResultDetails(resultClients) {
  const names = resultClients.map((result) => clients.find((client) => client.id === result.id).product);
  document.getElementById("doneClients").textContent = names.join(", ");
  document.getElementById("doneConfig").textContent = resultClients.map((result) => result.configPath).join("; ");
  document.getElementById("doneKey").textContent = resultClients.map((result) => result.keyPath).filter(Boolean).join("; ") || "Removed";
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

document.getElementById("uninstallButton").addEventListener("click", async () => {
  const button = document.getElementById("uninstallButton");
  const installButton = document.getElementById("installButton");
  const error = document.getElementById("error");
  error.hidden = true;
  button.disabled = true;
  installButton.disabled = true;
  document.getElementById("resultMark").classList.remove("warning-mark");
  document.getElementById("progressTitle").textContent = "Uninstalling Tally";
  document.getElementById("progressMessage").textContent = "Removing local credentials and hooks.";
  document.getElementById("status").textContent = "Uninstalling";
  show("installing");
  try {
    const result = await api("/api/uninstall", {
      clients: selectedClients(),
    });
    sessionStorage.removeItem("tallyInstallerToken");
    document.getElementById("doneTitle").textContent = "Tally is uninstalled";
    document.getElementById("doneMessage").textContent = "Hooks and local credentials were removed from this machine.";
    showResultDetails(result.clients);
    document.getElementById("warning").hidden = true;
    document.getElementById("retryButton").hidden = true;
    document.getElementById("status").textContent = "Uninstalled";
    show("done");
  } catch (requestError) {
    error.textContent = requestError.message;
    error.hidden = false;
    button.disabled = false;
    installButton.disabled = false;
    document.getElementById("status").textContent = "Update";
    show("setup");
  }
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
document.getElementById("doneCancelButton").addEventListener("click", shutdown);

load().catch((loadError) => {
  document.getElementById("status").textContent = "Unavailable";
  const error = document.getElementById("error");
  error.textContent = loadError.message;
  error.hidden = false;
  show("setup");
});
