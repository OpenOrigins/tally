const sections = ["setup", "installing", "done"];
const tokenFromHash = new URLSearchParams(window.location.hash.slice(1)).get("token");
if (tokenFromHash) {
  sessionStorage.setItem("tallyInstallerToken", tokenFromHash);
  history.replaceState(null, "", window.location.pathname);
}
const token = sessionStorage.getItem("tallyInstallerToken") || "";

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
  document.title = `Tally for ${status.product}`;
  document.getElementById("title").textContent = `Connect Tally to ${status.product}`;
  document.getElementById("configPath").textContent = status.configPath;
  document.getElementById("customConfigPath").value = status.configPath;
  document.getElementById("installStatus").textContent = status.installed ? "Installed - refresh available" : "Ready";
  document.getElementById("apiUrl").value = status.defaultApiUrl;
  document.getElementById("status").textContent = status.installed ? "Update" : "Setup";
  document.getElementById("uninstallButton").hidden = !status.installed;
  show("setup");
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
    const result = await api("/api/install", {
      apiKey: apiKeyInput.value,
      apiUrl: document.getElementById("apiUrl").value,
      configPath: document.getElementById("customConfigPath").value,
    });
    apiKeyInput.value = "";
    document.getElementById("doneConfig").textContent = result.configPath;
    document.getElementById("doneKey").textContent = result.keyPath;
    if (result.connected) {
      sessionStorage.removeItem("tallyInstallerToken");
      document.getElementById("doneTitle").textContent = "Tally is connected";
      document.getElementById("doneMessage").textContent = "Hooks are installed and the OpenOrigins dashboard confirmed this client.";
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
      configPath: document.getElementById("customConfigPath").value,
    });
    sessionStorage.removeItem("tallyInstallerToken");
    document.getElementById("doneTitle").textContent = "Tally is uninstalled";
    document.getElementById("doneMessage").textContent = "Hooks and local credentials were removed from this machine.";
    document.getElementById("doneConfig").textContent = result.configPath;
    document.getElementById("doneKey").textContent = "Removed";
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
