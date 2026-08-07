use fs2::FileExt;
use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

mod installer_gui;

pub use installer_gui::{run_installer_gui, GuiConfig};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const DEFAULT_API_URL: &str = "https://api.dev2.openorigins.com/v1/tally/logs";
const HANDSHAKE_PATH: &str = "/v1/tally/onboarding/client-connected";

pub fn should_open_gui_without_args() -> bool {
    if cfg!(windows) {
        return true;
    }
    env::current_exe()
        .map(|path| executable_is_in_app_bundle(&path))
        .unwrap_or(false)
}

fn executable_is_in_app_bundle(path: &Path) -> bool {
    path.ancestors()
        .any(|ancestor| ancestor.extension().and_then(|value| value.to_str()) == Some("app"))
}

pub struct InstallOptions {
    pub api_key: String,
    pub api_url: String,
}

#[derive(Debug)]
pub struct InstallReport {
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub logs_path: PathBuf,
    pub installed_binary_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub handshake_error: Option<String>,
}

pub struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
    #[cfg(unix)]
    mode: Option<u32>,
}

impl FileSnapshot {
    pub fn capture(path: &Path) -> io::Result<Self> {
        let contents = match fs::read(path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        #[cfg(unix)]
        let mode = if contents.is_some() {
            use std::os::unix::fs::PermissionsExt;
            Some(fs::metadata(path)?.permissions().mode() & 0o777)
        } else {
            None
        };
        Ok(Self {
            path: path.to_path_buf(),
            contents,
            #[cfg(unix)]
            mode,
        })
    }

    pub fn restore(&self) -> io::Result<()> {
        match &self.contents {
            Some(contents) => {
                let mode = {
                    #[cfg(unix)]
                    {
                        self.mode.unwrap_or(0o600)
                    }
                    #[cfg(not(unix))]
                    {
                        0o600
                    }
                };
                atomic_write(&self.path, contents, mode)
            }
            None => match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        }
    }
}

pub fn parse_install_options<I>(args: I, product: &str) -> Result<InstallOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut api_key = None;
    let mut api_url = None;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--api-key" => {
                api_key = Some(args.next().ok_or("--api-key requires a value")?);
            }
            "--api-url" => {
                api_url = Some(args.next().ok_or("--api-url requires a value")?);
            }
            _ if argument.starts_with("--api-key=") => {
                api_key = Some(argument["--api-key=".len()..].to_string());
            }
            _ if argument.starts_with("--api-url=") => {
                api_url = Some(argument["--api-url=".len()..].to_string());
            }
            _ => return Err(format!("unknown install option: {argument}").into()),
        }
    }

    let api_key = match api_key.map(|key| key.trim().to_string()) {
        Some(key) if !key.is_empty() => key,
        _ => prompt_api_key(product)?,
    };
    install_options(api_key, api_url)
}

pub fn install_options(api_key: String, api_url: Option<String>) -> Result<InstallOptions> {
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("Agent API key cannot be empty".into());
    }
    if api_key.len() > 4096 || api_key.chars().any(char::is_control) {
        return Err("Agent API key has an invalid format".into());
    }
    let api_url = api_url
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_API_URL.to_string());
    if api_url.len() > 2048 {
        return Err("API URL is too long".into());
    }
    validate_api_url(&api_url)?;
    Ok(InstallOptions { api_key, api_url })
}

fn prompt_api_key(product: &str) -> Result<String> {
    eprintln!(
        "Install Tally for {product}. Generate an Agent API key in the OpenOrigins dashboard, then paste it here."
    );
    let key = if io::stdin().is_terminal() {
        rpassword::prompt_password("Agent API key: ")?
    } else {
        #[cfg(target_os = "macos")]
        {
            prompt_macos_api_key()?
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Err("Agent API key is required; run install --api-key <agent-api-key>".into());
        }
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("Agent API key cannot be empty".into());
    }
    Ok(key)
}

#[cfg(target_os = "macos")]
fn prompt_macos_api_key() -> Result<String> {
    let output = Command::new("osascript")
        .args([
            "-e",
            "display dialog \"Paste the Agent API key from the OpenOrigins dashboard.\" default answer \"\" with hidden answer buttons {\"Cancel\", \"Install\"} default button \"Install\" with title \"Tally Installer\"",
        ])
        .output()?;
    if !output.status.success() {
        return Err("Agent API key entry was cancelled".into());
    }
    let output = String::from_utf8(output.stdout)?;
    output
        .split("text returned:")
        .nth(1)
        .map(str::trim)
        .map(str::to_string)
        .ok_or_else(|| "could not read Agent API key from the installer dialog".into())
}

pub fn validate_api_url(api_url: &str) -> Result<()> {
    let parsed = Url::parse(api_url)?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("API URL must be an absolute HTTP or HTTPS URL".into());
    }
    Ok(())
}

pub fn api_key_path(state_dir: &Path) -> PathBuf {
    state_dir.join("api_key.txt")
}

pub fn config_path(state_dir: &Path) -> PathBuf {
    state_dir.join("config.json")
}

pub fn write_credentials(state_dir: &Path, options: &InstallOptions) -> Result<()> {
    let key_path = api_key_path(state_dir);
    let config_path = config_path(state_dir);
    let key_snapshot = FileSnapshot::capture(&key_path)?;
    let config_snapshot = FileSnapshot::capture(&config_path)?;
    let result = (|| -> Result<()> {
        create_private_dir(state_dir)?;
        atomic_write(&key_path, options.api_key.as_bytes(), 0o600)?;
        let config = format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({"apiUrl": options.api_url}))?
        );
        atomic_write(&config_path, config.as_bytes(), 0o600)?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = key_snapshot.restore();
        let _ = config_snapshot.restore();
        return Err(error);
    }
    Ok(())
}

pub fn install_executable(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() && fs::canonicalize(source).ok() == fs::canonicalize(destination).ok() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        create_private_dir(parent)?;
    }
    atomic_write(destination, &fs::read(source)?, 0o755)?;
    Ok(())
}

pub fn notify_client_connected(
    api_key: &str,
    api_url: &str,
    source: &str,
) -> std::result::Result<(), String> {
    let endpoint = endpoint_for(api_url, HANDSHAKE_PATH).map_err(|error| error.to_string())?;
    let body = serde_json::to_string(&json!({"source": source})).map_err(|e| e.to_string())?;
    post_json(&endpoint, api_key, &body)
}

pub fn handshake_warning(error: &str) -> String {
    format!(
        "Automatic dashboard connection failed: {error}. Local logging is installed and will continue offline. Use \"Mark connected manually\" in the dashboard if needed."
    )
}

pub fn enqueue_record(state_dir: &Path, record_path: &Path, executable: &Path) -> Result<()> {
    if env_disabled("TALLY_FORWARDING_ENABLED") || !api_key_path(state_dir).exists() {
        return Ok(());
    }
    let queue_dir = state_dir.join("forward-queue");
    create_private_dir(&queue_dir)?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = record_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let queued = queue_dir.join(format!("{suffix}-{}-{name}", std::process::id()));
    atomic_write(&queued, &fs::read(record_path)?, 0o600)?;

    let mut command = Command::new(executable);
    command
        .arg("forward-pending")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.spawn()?;
    Ok(())
}

pub fn forward_pending(state_dir: &Path) -> Result<()> {
    let queue_dir = state_dir.join("forward-queue");
    if !queue_dir.exists() {
        return Ok(());
    }
    create_private_dir(state_dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(state_dir.join("forward.lock"))?;
    lock.lock_exclusive()?;

    let api_key = fs::read_to_string(api_key_path(state_dir))?
        .trim()
        .to_string();
    if api_key.is_empty() {
        return Err("stored Agent API key is empty".into());
    }
    let config: Value = serde_json::from_str(&fs::read_to_string(config_path(state_dir))?)?;
    let api_url = config["apiUrl"]
        .as_str()
        .ok_or("stored config does not contain apiUrl")?;
    validate_api_url(api_url)?;

    let mut records = fs::read_dir(&queue_dir)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    records.sort();
    for path in records {
        let body = fs::read_to_string(&path)?;
        serde_json::from_str::<Value>(&body)?;
        if let Err(error) = post_json(api_url, &api_key, &body) {
            write_forward_status(state_dir, false, Some(&error))?;
            return Err(error.into());
        }
        fs::remove_file(path)?;
    }
    write_forward_status(state_dir, true, None)?;
    FileExt::unlock(&lock)?;
    Ok(())
}

fn endpoint_for(api_url: &str, path: &str) -> Result<String> {
    let mut url = Url::parse(api_url)?;
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn post_json(url: &str, api_key: &str, body: &str) -> std::result::Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(5))
        .timeout_write(Duration::from_secs(5))
        .build();
    match agent
        .post(url)
        .set("x-api-key", api_key)
        .set("content-type", "application/json")
        .send_string(body)
    {
        Ok(response) if (200..300).contains(&response.status()) => {
            response_body_error(response.into_string().unwrap_or_default().trim())
        }
        Ok(response) => Err(format!("server returned HTTP {}", response.status())),
        Err(ureq::Error::Status(status, _)) => Err(format!("server returned HTTP {status}")),
        Err(ureq::Error::Transport(error)) => Err(error.to_string()),
    }
}

fn response_body_error(body: &str) -> std::result::Result<(), String> {
    if body.is_empty() {
        return Ok(());
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Ok(());
    };
    let status_code = value["status_code"]
        .as_i64()
        .or_else(|| value["statusCode"].as_i64())
        .unwrap_or_default();
    if status_code < 400 {
        return Ok(());
    }
    let code = value["code"].as_str().unwrap_or("api_error");
    let message = value["message"].as_str().unwrap_or(code);
    Err(format!("server returned {status_code}: {message}"))
}

fn write_forward_status(state_dir: &Path, ok: bool, error: Option<&str>) -> Result<()> {
    let updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let status = format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "ok": ok,
            "last_error": error,
            "updated_at_unix": updated_at,
        }))?
    );
    atomic_write(
        &state_dir.join("forwarding-status.json"),
        status.as_bytes(),
        0o600,
    )?;
    Ok(())
}

fn env_disabled(key: &str) -> bool {
    matches!(
        env::var(key).ok().as_deref(),
        Some("0" | "false" | "False" | "no")
    )
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, contents: &[u8], _mode: u32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_extension(format!("tmp-{}-{suffix}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(_mode);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(_mode))?;
    }
    drop(file);
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{executable_is_in_app_bundle, response_body_error};
    use std::path::Path;

    #[test]
    fn identifies_app_bundle_executables() {
        assert!(executable_is_in_app_bundle(Path::new(
            "/Applications/Tally Codex.app/Contents/MacOS/tally-codex"
        )));
        assert!(!executable_is_in_app_bundle(Path::new(
            "/usr/local/bin/tally-codex"
        )));
    }

    #[test]
    fn accepts_empty_or_success_response_bodies() {
        assert!(response_body_error("").is_ok());
        assert!(response_body_error(r#"{"status_code":200,"ok":true}"#).is_ok());
        assert!(response_body_error("accepted").is_ok());
    }

    #[test]
    fn rejects_successful_http_response_with_error_body() {
        let error = response_body_error(
            r#"{"billingSetupRequired":true,"code":"billing_setup_required","message":"billing_setup_required: Add a payment method before ingesting Agent Logs","status_code":402}"#,
        )
        .unwrap_err();
        assert!(error.contains("402"));
        assert!(error.contains("billing_setup_required"));
    }
}
