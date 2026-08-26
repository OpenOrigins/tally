use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

pub mod agent_runtime;
mod installer_gui;
mod server_evidence;

pub use installer_gui::{run_installer_gui, GuiClient};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const DEFAULT_API_URL: &str = match option_env!("TALLY_DEFAULT_API_URL") {
    Some(url) => url,
    None => "https://api.prod.openorigins.com/v1/tally/logs",
};
pub const DEFAULT_HEARTBEAT_INTERVAL_SECONDS: u64 = 600;
pub const DEFAULT_PRIVATE_RETENTION_DAYS: u64 = 30;
pub const DEFAULT_PRIVATE_STORAGE_LIMIT_MIB: u64 = 256;
const HANDSHAKE_PATH: &str = "/v1/tally/onboarding/client-connected";
const TALLY_DATA_MARKER: &str = ".openorigins-tally-data";
const FORWARD_CLAIM_STALE_AFTER: Duration = Duration::from_secs(30);
const FORWARD_QUEUE_VERSION: u64 = 1;
const STORAGE_GC_INTERVAL: Duration = Duration::from_secs(60 * 60);
#[cfg(target_os = "macos")]
const MACOS_HOOK_HELPER: &str = "tally-hook";

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

pub fn installation_source_executable() -> Result<PathBuf> {
    let executable = env::current_exe()?;
    #[cfg(target_os = "macos")]
    if let Some(app) = executable
        .ancestors()
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("app"))
    {
        let helper = app.join("Contents").join("Helpers").join(MACOS_HOOK_HELPER);
        if !helper.is_file() {
            return Err(format!("signed hook helper is missing from {}", helper.display()).into());
        }
        return Ok(helper);
    }
    Ok(executable)
}

#[derive(Clone)]
pub struct InstallOptions {
    pub api_key: String,
    pub api_url: String,
    pub config_path: Option<PathBuf>,
}

#[derive(Debug)]
pub struct InstallReport {
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub logs_path: PathBuf,
    pub installed_binary_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub handshake_error: Option<String>,
    pub approval_required: bool,
    pub approval_instructions: Option<String>,
    pub client_version: Option<String>,
}

#[derive(Debug)]
pub struct UninstallReport {
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub logs_path: PathBuf,
    pub queue_path: PathBuf,
    pub data_removed: bool,
}

pub fn mark_tally_data_directory(path: &Path) -> Result<()> {
    create_private_dir(path)?;
    let marker = path.join(TALLY_DATA_MARKER);
    if !marker.is_file() {
        atomic_write(&marker, b"Owned by OpenOrigins Tally.\n", 0o600)?;
    }
    Ok(())
}

pub fn remove_tally_data(state_dir: &Path, logs_path: &Path) -> Result<()> {
    if state_dir.exists() {
        let valid_state_path = state_dir.file_name().and_then(|name| name.to_str())
            == Some(".state")
            && state_dir
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                == Some("logs")
            && state_dir
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                == Some("tally");
        if !valid_state_path {
            return Err(format!(
                "refusing to delete unexpected Tally state path {}",
                state_dir.display()
            )
            .into());
        }
    }

    if logs_path.exists() {
        let is_default_layout = logs_path.file_name().and_then(|name| name.to_str())
            == Some("logs")
            && logs_path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".tally-"));
        if !is_default_layout && !logs_path.join(TALLY_DATA_MARKER).is_file() {
            return Err(format!(
                "refusing to delete unmarked log directory {}; remove it manually if this path is intentional",
                logs_path.display()
            )
            .into());
        }
    }

    if state_dir.exists() {
        fs::remove_dir_all(state_dir)?;
    }
    if logs_path.exists() {
        fs::remove_dir_all(logs_path)?;
    }
    Ok(())
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

pub fn restore_snapshots(snapshots: &[&FileSnapshot]) -> io::Result<()> {
    let errors = snapshots
        .iter()
        .filter_map(|snapshot| snapshot.restore().err().map(|error| error.to_string()))
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}

pub fn install_error_with_rollback(
    error: Box<dyn std::error::Error>,
    snapshots: &[&FileSnapshot],
) -> Box<dyn std::error::Error> {
    match restore_snapshots(snapshots) {
        Ok(()) => error,
        Err(rollback_error) => {
            format!("{error}; restoring the previous installation also failed: {rollback_error}")
                .into()
        }
    }
}

pub fn parse_install_options<I>(args: I, product: &str) -> Result<InstallOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut api_key = None;
    let mut api_url = None;
    let mut config_path = None;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--api-key" => {
                api_key = Some(args.next().ok_or("--api-key requires a value")?);
            }
            "--api-url" => {
                api_url = Some(args.next().ok_or("--api-url requires a value")?);
            }
            "--config-path" => {
                config_path = Some(args.next().ok_or("--config-path requires a value")?);
            }
            _ if argument.starts_with("--api-key=") => {
                api_key = Some(argument["--api-key=".len()..].to_string());
            }
            _ if argument.starts_with("--api-url=") => {
                api_url = Some(argument["--api-url=".len()..].to_string());
            }
            _ if argument.starts_with("--config-path=") => {
                config_path = Some(argument["--config-path=".len()..].to_string());
            }
            _ => return Err(format!("unknown install option: {argument}").into()),
        }
    }

    let api_key = match api_key.map(|key| key.trim().to_string()) {
        Some(key) if !key.is_empty() => key,
        _ => prompt_api_key(product)?,
    };
    install_options(api_key, api_url, config_path)
}

pub fn parse_config_path_options<I>(args: I) -> Result<Option<PathBuf>>
where
    I: IntoIterator<Item = String>,
{
    let mut config_path = None;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--config-path" => {
                config_path = Some(args.next().ok_or("--config-path requires a value")?);
            }
            _ if argument.starts_with("--config-path=") => {
                config_path = Some(argument["--config-path=".len()..].to_string());
            }
            _ => return Err(format!("unknown option: {argument}").into()),
        }
    }
    normalize_config_path(config_path)
}

pub fn install_options(
    api_key: String,
    api_url: Option<String>,
    config_path: Option<String>,
) -> Result<InstallOptions> {
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
    let config_path = normalize_config_path(config_path)?;
    Ok(InstallOptions {
        api_key,
        api_url,
        config_path,
    })
}

fn normalize_config_path(config_path: Option<String>) -> Result<Option<PathBuf>> {
    let Some(config_path) = config_path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
    else {
        return Ok(None);
    };
    if config_path.len() > 4096 || config_path.chars().any(char::is_control) {
        return Err("Configuration path has an invalid format".into());
    }
    let path = expand_home_path(&config_path);
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    };
    Ok(Some(path))
}

fn expand_home_path(value: &str) -> PathBuf {
    if value == "~" {
        PathBuf::from(home_dir())
    } else if let Some(rest) = value.strip_prefix("~/") {
        PathBuf::from(home_dir()).join(rest)
    } else {
        PathBuf::from(value)
    }
}

fn home_dir() -> String {
    env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .or_else(|_| {
            Ok::<_, env::VarError>(format!(
                "{}{}",
                env::var("HOMEDRIVE")?,
                env::var("HOMEPATH")?
            ))
        })
        .unwrap_or_else(|_| ".".to_string())
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
        return Err(install_error_with_rollback(
            error,
            &[&key_snapshot, &config_snapshot],
        ));
    }
    Ok(())
}

pub fn install_executable(source: &Path, destination: &Path) -> Result<()> {
    let verify_signature = is_bundled_macos_hook_helper(source);
    if verify_signature {
        verify_macos_code_signature(source)?;
    }
    if destination.exists() && fs::canonicalize(source).ok() == fs::canonicalize(destination).ok() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        create_private_dir(parent)?;
    }
    atomic_write(destination, &fs::read(source)?, 0o755)?;
    if verify_signature {
        verify_macos_code_signature(destination)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn is_bundled_macos_hook_helper(path: &Path) -> bool {
    path.file_name().and_then(|value| value.to_str()) == Some(MACOS_HOOK_HELPER)
        && path
            .ancestors()
            .any(|parent| parent.extension().and_then(|value| value.to_str()) == Some("app"))
}

#[cfg(not(target_os = "macos"))]
fn is_bundled_macos_hook_helper(_path: &Path) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn verify_macos_code_signature(path: &Path) -> Result<()> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--verbose=2"])
        .arg(path)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(format!(
        "macOS rejected the hook code signature at {}{}",
        path.display(),
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    )
    .into())
}

#[cfg(not(target_os = "macos"))]
fn verify_macos_code_signature(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn installed_executable_path(config_path: &Path, binary_name: &str) -> PathBuf {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    #[cfg(windows)]
    {
        let local_app_data = env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home_dir()).join("AppData").join("Local"));
        let normalized_config = config_path
            .to_string_lossy()
            .replace('/', "\\")
            .to_lowercase();
        let digest = format!("{:x}", Sha256::digest(normalized_config.as_bytes()));
        return local_app_data
            .join("Programs")
            .join("OpenOrigins")
            .join("Tally")
            .join(binary_name.trim_start_matches("tally-"))
            .join(&digest[..12])
            .join(format!("{binary_name}{suffix}"));
    }
    #[cfg(not(windows))]
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tally")
        .join("bin")
        .join(format!("{binary_name}{suffix}"))
}

pub fn legacy_installed_executable_path(config_path: &Path, binary_name: &str) -> PathBuf {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tally")
        .join("bin")
        .join(format!("{binary_name}{suffix}"))
}

pub fn remove_legacy_installed_executable(config_path: &Path, binary_name: &str) -> Result<()> {
    let legacy = legacy_installed_executable_path(config_path, binary_name);
    let current = installed_executable_path(config_path, binary_name);
    if legacy == current {
        return Ok(());
    }
    match fs::remove_file(legacy) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
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
    let contents = fs::read(record_path)?;
    let queue_name = forward_queue_name(&contents);
    let record: Value = serde_json::from_slice(&contents)?;
    let log_root = log_root_for_record_path(record_path);
    let private_paths = log_root
        .as_deref()
        .map(|root| private_paths_for_record(root, &record))
        .unwrap_or_default();
    let envelope = json!({
        "tally_forward_queue_version": FORWARD_QUEUE_VERSION,
        "record": record,
        "local": {
            "log_root": log_root,
            "private_paths": private_paths,
        },
    });
    let queued_contents = format!("{}\n", serde_json::to_string_pretty(&envelope)?);
    let mut durable_envelope = false;
    if !forward_record_is_pending(&queue_dir, &queue_name)? {
        let queued = queue_dir.join(&queue_name);
        if let Err(error) = atomic_write(&queued, queued_contents.as_bytes(), 0o600) {
            if !forward_record_is_pending(&queue_dir, &queue_name)? {
                return Err(error.into());
            }
        } else {
            durable_envelope = true;
        }
    } else {
        durable_envelope = pending_forward_record_is_envelope(&queue_dir, &queue_name)?;
    }
    if durable_envelope {
        match fs::remove_file(record_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "Warning: queued record but could not remove redundant local copy {}: {error}",
                record_path.display()
            ),
        }
    }

    spawn_background(executable, &["forward-pending"])
}

fn forward_queue_name(contents: &[u8]) -> String {
    format!("{:x}.json", Sha256::digest(contents))
}

fn forward_record_is_pending(queue_dir: &Path, queue_name: &str) -> io::Result<bool> {
    if queue_dir.join(queue_name).exists() {
        return Ok(true);
    }
    let claim_prefix = format!("{queue_name}.sending-");
    Ok(fs::read_dir(queue_dir)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .any(|name| name.starts_with(&claim_prefix)))
}

fn forward_queue_file_is_envelope(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
        .is_some_and(|value| {
            value["tally_forward_queue_version"].as_u64() == Some(FORWARD_QUEUE_VERSION)
                && value.get("record").is_some()
        })
}

fn pending_forward_record_is_envelope(queue_dir: &Path, queue_name: &str) -> io::Result<bool> {
    let queued = queue_dir.join(queue_name);
    if queued.is_file() {
        return Ok(forward_queue_file_is_envelope(&queued));
    }
    let claim_prefix = format!("{queue_name}.sending-");
    for entry in fs::read_dir(queue_dir)?.filter_map(std::result::Result::ok) {
        let path = entry.path();
        let matches_claim = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&claim_prefix));
        if matches_claim {
            return Ok(forward_queue_file_is_envelope(&path));
        }
    }
    // A matching item observed immediately before this check can disappear only
    // after the forwarder has accepted it, so its original local copy is redundant.
    Ok(true)
}

struct QueuedRecord {
    body: String,
    log_root: Option<PathBuf>,
}

fn decode_forward_queue_item(contents: &str) -> Result<QueuedRecord> {
    let value: Value = serde_json::from_str(contents)?;
    let Some(version) = value.get("tally_forward_queue_version") else {
        return Ok(QueuedRecord {
            body: contents.to_string(),
            log_root: None,
        });
    };
    if version.as_u64() != Some(FORWARD_QUEUE_VERSION) {
        return Err(format!("unsupported forwarding queue version: {version}").into());
    }
    let record = value
        .get("record")
        .ok_or("forwarding queue envelope does not contain a record")?;
    let log_root = value["local"]["log_root"].as_str().map(PathBuf::from);
    Ok(QueuedRecord {
        body: serde_json::to_string(record)?,
        log_root,
    })
}

pub fn spawn_background(executable: &Path, args: &[&str]) -> Result<()> {
    #[cfg(windows)]
    spawn_background_windows(executable, args)?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new(executable);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Hook runners may terminate their process group after each hook exits.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn()?;
    }

    #[cfg(all(not(unix), not(windows)))]
    Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(())
}

#[cfg(windows)]
fn spawn_background_windows(executable: &Path, args: &[&str]) -> io::Result<()> {
    use std::iter;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let application = executable
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let mut command_line = windows_command_line(executable, args);
    let mut startup_info: STARTUPINFOW = unsafe { zeroed() };
    startup_info.cb = size_of::<STARTUPINFOW>() as u32;
    let mut process_info: PROCESS_INFORMATION = unsafe { zeroed() };

    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_NO_WINDOW,
            ptr::null(),
            ptr::null(),
            &startup_info,
            &mut process_info,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }

    unsafe {
        CloseHandle(process_info.hThread);
        CloseHandle(process_info.hProcess);
    }
    Ok(())
}

#[cfg(windows)]
fn windows_command_line(executable: &Path, args: &[&str]) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    let mut command_line = Vec::new();
    append_windows_argument(
        &mut command_line,
        &executable.as_os_str().encode_wide().collect::<Vec<_>>(),
    );
    for arg in args {
        command_line.push(b' ' as u16);
        append_windows_argument(&mut command_line, &arg.encode_utf16().collect::<Vec<_>>());
    }
    command_line.push(0);
    command_line
}

#[cfg(windows)]
fn append_windows_argument(command_line: &mut Vec<u16>, argument: &[u16]) {
    const BACKSLASH: u16 = b'\\' as u16;
    const QUOTE: u16 = b'"' as u16;

    let needs_quotes = argument.is_empty()
        || argument
            .iter()
            .any(|character| matches!(*character, 0x09 | 0x20 | QUOTE));
    if !needs_quotes {
        command_line.extend_from_slice(argument);
        return;
    }

    command_line.push(QUOTE);
    let mut backslashes = 0;
    for character in argument {
        if *character == BACKSLASH {
            backslashes += 1;
            continue;
        }
        if *character == QUOTE {
            command_line.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2 + 1));
        } else {
            command_line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        }
        backslashes = 0;
        command_line.push(*character);
    }
    command_line.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    command_line.push(QUOTE);
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
    recover_stale_forward_claims(&queue_dir)?;

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
        .filter(|path| path.is_file() && path.extension().is_some_and(|value| value == "json"))
        .collect::<Vec<_>>();
    records.sort();
    let mut log_roots = BTreeSet::new();
    for path in records {
        let Some(claimed) = claim_forward_record(&path)? else {
            continue;
        };
        let body = match fs::read_to_string(&claimed) {
            Ok(body) => body,
            Err(error) => {
                restore_forward_claim(&path, &claimed)?;
                return Err(error.into());
            }
        };
        let queued = match decode_forward_queue_item(&body) {
            Ok(queued) => queued,
            Err(error) => {
                restore_forward_claim(&path, &claimed)?;
                return Err(error);
            }
        };
        if let Err(error) = post_json(api_url, &api_key, &queued.body) {
            restore_forward_claim(&path, &claimed)?;
            write_forward_status(state_dir, false, Some(&error))?;
            return Err(error.into());
        }
        fs::remove_file(claimed)?;
        if let Some(log_root) = queued.log_root {
            log_roots.insert(log_root);
        }
    }
    for log_root in log_roots {
        if let Err(error) = maybe_prune_local_storage(&log_root, state_dir, true) {
            eprintln!(
                "Warning: records were forwarded but local evidence cleanup failed for {}: {error}",
                log_root.display()
            );
        }
    }
    write_forward_status(state_dir, true, None)?;
    FileExt::unlock(&lock)?;
    Ok(())
}

fn claim_forward_record(path: &Path) -> io::Result<Option<PathBuf>> {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    let claimed = path.with_file_name(format!("{name}.sending-{}", std::process::id()));
    match fs::rename(path, &claimed) {
        Ok(()) => Ok(Some(claimed)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn restore_forward_claim(path: &Path, claimed: &Path) -> io::Result<()> {
    if path.exists() {
        fs::remove_file(claimed)
    } else {
        fs::rename(claimed, path)
    }
}

fn recover_stale_forward_claims(queue_dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(queue_dir)?.filter_map(std::result::Result::ok) {
        let claimed = entry.path();
        let Some(name) = claimed.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some((original_name, _)) = name.rsplit_once(".sending-") else {
            continue;
        };
        let stale = claimed
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed >= FORWARD_CLAIM_STALE_AFTER);
        if !stale {
            continue;
        }
        let original = queue_dir.join(original_name);
        restore_forward_claim(&original, &claimed)?;
    }
    Ok(())
}

pub(crate) fn maybe_prune_local_storage(
    log_root: &Path,
    forwarding_state_dir: &Path,
    force: bool,
) -> Result<()> {
    if !log_root.join(TALLY_DATA_MARKER).is_file() {
        return Ok(());
    }
    let state_dir = log_root.join("state");
    create_private_dir(&state_dir)?;
    let lock = open_private_lock(&state_dir.join("storage-gc.lock"))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if lock_is_contended(&error) => return Ok(()),
        Err(error) => return Err(error.into()),
    }

    let status_path = state_dir.join("storage-gc.json");
    let now = unix_now_seconds();
    let gc_interval = env::var("TALLY_STORAGE_GC_INTERVAL_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(STORAGE_GC_INTERVAL.as_secs());
    let previous = fs::read_to_string(&status_path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
        .and_then(|value| value["updated_at_unix"].as_u64())
        .unwrap_or_default();
    if !force && now.saturating_sub(previous) < gc_interval {
        FileExt::unlock(&lock)?;
        return Ok(());
    }

    let retention_seconds = configured_private_retention_seconds();
    let storage_limit_bytes = configured_private_storage_limit_bytes();
    let report = prune_local_storage(
        log_root,
        forwarding_state_dir,
        retention_seconds,
        storage_limit_bytes,
    )?;
    let status = format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "updated_at_unix": now,
            "retention_seconds": retention_seconds,
            "storage_limit_bytes": storage_limit_bytes,
            "removed_files": report.removed_files,
            "removed_bytes": report.removed_bytes,
            "retained_bytes": report.retained_bytes,
            "protected_files": report.protected_files,
        }))?
    );
    atomic_write(&status_path, status.as_bytes(), 0o600)?;
    FileExt::unlock(&lock)?;
    Ok(())
}

#[derive(Default)]
struct StoragePruneReport {
    removed_files: u64,
    removed_bytes: u64,
    retained_bytes: u64,
    protected_files: u64,
}

fn prune_local_storage(
    log_root: &Path,
    forwarding_state_dir: &Path,
    retention_seconds: u64,
    storage_limit_bytes: u64,
) -> Result<StoragePruneReport> {
    let private_root = log_root.join("private");
    let protected = protected_private_paths(log_root, forwarding_state_dir)?;
    let now = SystemTime::now();
    let managed_roots = [
        private_root.clone(),
        log_root.join("jsonl"),
        log_root.join("claude-stdio"),
        log_root.join("codex-stdio"),
        forwarding_state_dir.join("desktop-notifications"),
    ];
    let mut files = managed_roots
        .iter()
        .map(|root| files_below(root))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            if !metadata.is_file() {
                return None;
            }
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            Some((path, metadata.len(), modified))
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut report = StoragePruneReport {
        retained_bytes: files.iter().map(|(_, size, _)| *size).sum(),
        protected_files: files
            .iter()
            .filter(|(path, _, _)| protected.contains(path))
            .count() as u64,
        ..StoragePruneReport::default()
    };

    for (path, size, modified) in files {
        if protected.contains(&path) {
            continue;
        }
        let expired =
            now.duration_since(modified).unwrap_or_default().as_secs() >= retention_seconds;
        let over_limit = report.retained_bytes > storage_limit_bytes;
        if !expired && !over_limit {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                report.removed_files += 1;
                report.removed_bytes += size;
                report.retained_bytes = report.retained_bytes.saturating_sub(size);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for root in managed_roots {
        remove_empty_directories(&root, &root)?;
    }
    Ok(report)
}

fn protected_private_paths(
    log_root: &Path,
    forwarding_state_dir: &Path,
) -> Result<HashSet<PathBuf>> {
    let private_root = log_root.join("private");
    let mut protected = HashSet::new();
    for path in files_below(&log_root.join("tally"))? {
        collect_private_paths_from_file(log_root, &private_root, &path, &mut protected);
    }
    for path in files_below(&forwarding_state_dir.join("forward-queue"))? {
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&contents) else {
            continue;
        };
        if value["tally_forward_queue_version"].as_u64() == Some(FORWARD_QUEUE_VERSION) {
            if let Some(paths) = value["local"]["private_paths"].as_array() {
                for path in paths.iter().filter_map(Value::as_str).map(PathBuf::from) {
                    if path.starts_with(&private_root) {
                        protected.insert(path);
                    }
                }
            }
            if let Some(record) = value.get("record") {
                collect_private_paths(log_root, &private_root, record, &mut protected);
            }
        } else {
            collect_private_paths(log_root, &private_root, &value, &mut protected);
        }
    }
    Ok(protected)
}

fn collect_private_paths_from_file(
    log_root: &Path,
    private_root: &Path,
    path: &Path,
    protected: &mut HashSet<PathBuf>,
) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&contents) else {
        return;
    };
    collect_private_paths(log_root, private_root, &value, protected);
}

fn collect_private_paths(
    log_root: &Path,
    private_root: &Path,
    value: &Value,
    protected: &mut HashSet<PathBuf>,
) {
    match value {
        Value::String(value) => {
            if let Some(path) = resolve_private_uri(log_root, value) {
                if path.starts_with(private_root) {
                    protected.insert(path);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_private_paths(log_root, private_root, value, protected);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_private_paths(log_root, private_root, value, protected);
            }
        }
        _ => {}
    }
}

fn private_paths_for_record(log_root: &Path, record: &Value) -> Vec<PathBuf> {
    let private_root = log_root.join("private");
    let mut paths = HashSet::new();
    collect_private_paths(log_root, &private_root, record, &mut paths);
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort();
    paths
}

fn resolve_private_uri(log_root: &Path, uri: &str) -> Option<PathBuf> {
    let components = uri
        .strip_prefix("private://")?
        .split('/')
        .collect::<Vec<_>>();
    if components.iter().any(|part| {
        part.is_empty()
            || *part == "."
            || *part == ".."
            || part.contains('\\')
            || part.contains(':')
    }) {
        return None;
    }
    if components.len() == 2 && components[0] == "sha256" {
        let digest = components[1];
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        return Some(
            log_root
                .join("private")
                .join("objects")
                .join(&digest[..2])
                .join(format!("{digest}.json")),
        );
    }
    if components.len() == 3 {
        return Some(
            log_root
                .join("private")
                .join(components[0])
                .join(components[1])
                .join(components[2]),
        );
    }
    None
}

fn log_root_for_record_path(record_path: &Path) -> Option<PathBuf> {
    let source_dir = record_path.parent()?;
    let tally_dir = source_dir.parent()?;
    if tally_dir.file_name().and_then(|name| name.to_str()) != Some("tally") {
        return None;
    }
    tally_dir.parent().map(Path::to_path_buf)
}

fn files_below(root: &Path) -> io::Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                directories.push(path);
            } else {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn remove_empty_directories(root: &Path, directory: &Path) -> io::Result<bool> {
    if !directory.is_dir() {
        return Ok(false);
    }
    for entry in fs::read_dir(directory)?.filter_map(std::result::Result::ok) {
        if entry.file_type()?.is_dir() {
            remove_empty_directories(root, &entry.path())?;
        }
    }
    let empty = fs::read_dir(directory)?.next().is_none();
    if empty && directory != root {
        fs::remove_dir(directory)?;
    }
    Ok(empty)
}

fn configured_private_retention_seconds() -> u64 {
    env::var("TALLY_PRIVATE_RETENTION_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| {
            env::var("TALLY_PRIVATE_RETENTION_DAYS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_PRIVATE_RETENTION_DAYS)
                .saturating_mul(24 * 60 * 60)
        })
}

fn configured_private_storage_limit_bytes() -> u64 {
    env::var("TALLY_PRIVATE_STORAGE_LIMIT_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| {
            env::var("TALLY_PRIVATE_STORAGE_LIMIT_MIB")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_PRIVATE_STORAGE_LIMIT_MIB)
                .saturating_mul(1024 * 1024)
        })
}

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn heartbeat_interval_seconds(requested_seconds: u64) -> u64 {
    requested_seconds.max(DEFAULT_HEARTBEAT_INTERVAL_SECONDS)
}

pub fn record_agent_activity(
    state_dir: &Path,
    agent_id: &str,
    observed_at_unix_millis: u64,
) -> Result<()> {
    let (state_path, lock_path) = heartbeat_limiter_paths(state_dir, agent_id);
    create_private_dir(state_dir)?;
    let lock = open_private_lock(&lock_path)?;
    lock.lock_exclusive()?;

    let mut state = read_heartbeat_limiter_state(&state_path)?;
    let previous = state["last_activity_unix_millis"]
        .as_u64()
        .unwrap_or_default();
    state["last_activity_unix_millis"] = Value::from(previous.max(observed_at_unix_millis));
    write_heartbeat_limiter_state(&state_path, &state)?;
    FileExt::unlock(&lock)?;
    Ok(())
}

pub fn claim_agent_heartbeat(
    state_dir: &Path,
    agent_id: &str,
    now_unix_millis: u64,
    requested_interval_seconds: u64,
) -> Result<Option<u64>> {
    let interval_seconds = heartbeat_interval_seconds(requested_interval_seconds);
    let interval_millis = interval_seconds.saturating_mul(1_000);
    let (state_path, lock_path) = heartbeat_limiter_paths(state_dir, agent_id);
    create_private_dir(state_dir)?;
    let lock = open_private_lock(&lock_path)?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if lock_is_contended(&error) => return Ok(None),
        Err(error) => return Err(error.into()),
    }

    let mut state = read_heartbeat_limiter_state(&state_path)?;
    let last_activity = state["last_activity_unix_millis"]
        .as_u64()
        .unwrap_or_default();
    let last_heartbeat = state["last_heartbeat_unix_millis"]
        .as_u64()
        .unwrap_or_default();
    let last_signal = last_activity.max(last_heartbeat);
    if last_signal > 0 && now_unix_millis.saturating_sub(last_signal) < interval_millis {
        FileExt::unlock(&lock)?;
        return Ok(None);
    }

    state["last_heartbeat_unix_millis"] = Value::from(now_unix_millis);
    write_heartbeat_limiter_state(&state_path, &state)?;
    FileExt::unlock(&lock)?;
    Ok(Some(interval_seconds))
}

fn heartbeat_limiter_paths(state_dir: &Path, agent_id: &str) -> (PathBuf, PathBuf) {
    let digest = format!("{:x}", Sha256::digest(agent_id.as_bytes()));
    let stem = format!("agent-heartbeat.{}", &digest[..16]);
    (
        state_dir.join(format!("{stem}.json")),
        state_dir.join(format!("{stem}.lock")),
    )
}

fn open_private_lock(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32) | Some(33)) {
        return true;
    }
    false
}

fn read_heartbeat_limiter_state(path: &Path) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(serde_json::from_str(&contents)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error.into()),
    }
}

fn write_heartbeat_limiter_state(path: &Path, state: &Value) -> Result<()> {
    let contents = format!("{}\n", serde_json::to_string_pretty(state)?);
    atomic_write(path, contents.as_bytes(), 0o600)?;
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
    if let Err(error) = replace_file(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::spawn_background;
    use super::{
        atomic_write, claim_agent_heartbeat, claim_forward_record, decode_forward_queue_item,
        executable_is_in_app_bundle, forward_queue_name, forward_record_is_pending,
        heartbeat_interval_seconds, install_options, mark_tally_data_directory,
        pending_forward_record_is_envelope, prune_local_storage, record_agent_activity,
        remove_tally_data, resolve_private_uri, response_body_error, restore_forward_claim,
        restore_snapshots, FileSnapshot, DEFAULT_API_URL, DEFAULT_HEARTBEAT_INTERVAL_SECONDS,
        DEFAULT_PRIVATE_RETENTION_DAYS, DEFAULT_PRIVATE_STORAGE_LIMIT_MIB,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_directory(label: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "tally-common-{label}-{}-{suffix}",
            std::process::id()
        ))
    }

    #[test]
    fn atomic_write_replaces_existing_content() {
        let directory = test_directory("atomic-write");
        let path = directory.join("state.json");

        atomic_write(&path, b"first", 0o600).unwrap();
        atomic_write(&path, b"second", 0o600).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn snapshots_restore_existing_and_new_files() {
        let directory = test_directory("snapshots");
        let existing = directory.join("existing.txt");
        let new = directory.join("new.txt");
        atomic_write(&existing, b"original", 0o600).unwrap();
        let existing_snapshot = FileSnapshot::capture(&existing).unwrap();
        let new_snapshot = FileSnapshot::capture(&new).unwrap();

        atomic_write(&existing, b"changed", 0o600).unwrap();
        atomic_write(&new, b"created", 0o600).unwrap();
        restore_snapshots(&[&existing_snapshot, &new_snapshot]).unwrap();

        assert_eq!(fs::read(&existing).unwrap(), b"original");
        assert!(!new.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn identifies_app_bundle_executables() {
        assert!(executable_is_in_app_bundle(Path::new(
            "/Applications/Tally Codex.app/Contents/MacOS/tally-codex"
        )));
        assert!(!executable_is_in_app_bundle(Path::new(
            "/usr/local/bin/tally-codex"
        )));
    }

    #[cfg(windows)]
    #[test]
    fn quotes_background_process_command_line() {
        let command_line = super::windows_command_line(
            Path::new(r"C:\Program Files\Tally\tally.exe"),
            &["codex", "heartbeat-daemon", "session with spaces"],
        );
        assert_eq!(
            String::from_utf16(&command_line[..command_line.len() - 1]).unwrap(),
            r#""C:\Program Files\Tally\tally.exe" codex heartbeat-daemon "session with spaces""#
        );
        assert_eq!(command_line.last(), Some(&0));
    }

    #[cfg(unix)]
    #[test]
    fn starts_background_processes_in_a_new_session() {
        use std::thread;
        use std::time::{Duration, Instant};

        let output_path = test_directory("background-session");
        let output = output_path.to_string_lossy().into_owned();
        spawn_background(
            Path::new("/usr/bin/env"),
            &[
                "python3",
                "-c",
                "import os, sys; tmp = sys.argv[1] + '.tmp'; open(tmp, 'w').write(f'{os.getpid()} {os.getsid(0)}\\n'); os.replace(tmp, sys.argv[1])",
                &output,
            ],
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !output_path.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        let values = fs::read_to_string(&output_path).unwrap();
        let mut values = values.split_whitespace();
        let child_pid = values.next().unwrap().parse::<u32>().unwrap();
        let child_session = values.next().unwrap().parse::<u32>().unwrap();
        assert_eq!(child_pid, child_session);
        fs::remove_file(output_path).unwrap();
    }

    #[test]
    fn installer_uses_the_build_default_ingest() {
        let options = install_options("test-agent-key".to_string(), None, None).unwrap();
        assert_eq!(options.api_url, DEFAULT_API_URL);
        assert_eq!(
            options.api_url,
            option_env!("TALLY_DEFAULT_API_URL")
                .unwrap_or("https://api.prod.openorigins.com/v1/tally/logs")
        );
    }

    #[test]
    fn heartbeat_interval_has_a_ten_minute_minimum() {
        assert_eq!(heartbeat_interval_seconds(0), 600);
        assert_eq!(heartbeat_interval_seconds(60), 600);
        assert_eq!(heartbeat_interval_seconds(600), 600);
        assert_eq!(heartbeat_interval_seconds(900), 900);
        assert_eq!(DEFAULT_HEARTBEAT_INTERVAL_SECONDS, 600);
    }

    #[test]
    fn heartbeat_rate_limit_is_shared_by_agent() {
        let directory = test_directory("heartbeat-rate");
        record_agent_activity(&directory, "agent-a", 1_000).unwrap();

        assert_eq!(
            claim_agent_heartbeat(&directory, "agent-a", 600_999, 1).unwrap(),
            None
        );
        assert_eq!(
            claim_agent_heartbeat(&directory, "agent-a", 601_000, 1).unwrap(),
            Some(600)
        );
        assert_eq!(
            claim_agent_heartbeat(&directory, "agent-a", 1_200_999, 1).unwrap(),
            None
        );
        assert_eq!(
            claim_agent_heartbeat(&directory, "agent-a", 1_201_000, 1).unwrap(),
            Some(600)
        );
        assert_eq!(
            claim_agent_heartbeat(&directory, "agent-a", 500, 1).unwrap(),
            None
        );
        assert_eq!(
            claim_agent_heartbeat(&directory, "agent-b", 500, 1).unwrap(),
            Some(600)
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn forwarding_queue_coalesces_and_atomically_claims_records() {
        let directory = test_directory("forward-claim");
        fs::create_dir_all(&directory).unwrap();
        let body = br#"{"record_id":"rec-1"}"#;
        assert_eq!(forward_queue_name(body), forward_queue_name(body));
        assert_ne!(
            forward_queue_name(body),
            forward_queue_name(br#"{"record_id":"rec-2"}"#)
        );

        let queued = directory.join(forward_queue_name(body));
        fs::write(&queued, body).unwrap();
        let claimed = claim_forward_record(&queued).unwrap().unwrap();
        assert!(!queued.exists());
        assert!(claimed.exists());
        assert!(claim_forward_record(&queued).unwrap().is_none());
        assert!(forward_record_is_pending(
            &directory,
            queued.file_name().unwrap().to_str().unwrap()
        )
        .unwrap());
        assert!(!pending_forward_record_is_envelope(
            &directory,
            queued.file_name().unwrap().to_str().unwrap()
        )
        .unwrap());

        restore_forward_claim(&queued, &claimed).unwrap();
        assert!(queued.exists());
        assert!(!claimed.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn private_cache_prunes_only_unreachable_objects() {
        let directory = test_directory("private-gc");
        let log_root = directory.join("logs");
        let forwarding_state = directory.join("forwarding");
        mark_tally_data_directory(&log_root).unwrap();
        let protected = log_root
            .join("private/objects/aa")
            .join(format!("{}.json", "a".repeat(64)));
        let orphan = log_root
            .join("private/objects/bb")
            .join(format!("{}.json", "b".repeat(64)));
        atomic_write(&protected, b"protected", 0o600).unwrap();
        atomic_write(&orphan, b"orphan", 0o600).unwrap();
        let duplicate_jsonl = log_root.join("jsonl/debug.jsonl");
        atomic_write(&duplicate_jsonl, b"duplicate\n", 0o600).unwrap();
        let record = log_root.join("tally/test/000001_RECORD.json");
        atomic_write(
            &record,
            format!(
                "{{\"payload_uri\":\"private://sha256/{}\"}}\n",
                "a".repeat(64)
            )
            .as_bytes(),
            0o600,
        )
        .unwrap();

        let report = prune_local_storage(&log_root, &forwarding_state, 0, 0).unwrap();
        assert!(protected.exists());
        assert!(!orphan.exists());
        assert!(!duplicate_jsonl.exists());
        assert_eq!(report.protected_files, 1);

        fs::remove_file(record).unwrap();
        prune_local_storage(&log_root, &forwarding_state, 0, 0).unwrap();
        assert!(!protected.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn private_cache_defaults_are_bounded() {
        assert_eq!(DEFAULT_PRIVATE_RETENTION_DAYS, 30);
        assert_eq!(DEFAULT_PRIVATE_STORAGE_LIMIT_MIB, 256);
    }

    #[test]
    fn forward_queue_envelopes_keep_local_metadata_off_the_wire() {
        let queued = serde_json::json!({
            "tally_forward_queue_version": 1,
            "record": {"record_type": "ACTION_TAKEN", "record_id": "rec-1"},
            "local": {
                "log_root": "/private/device/logs",
                "private_paths": ["/private/device/logs/private/object.json"]
            }
        });
        let decoded = decode_forward_queue_item(&queued.to_string()).unwrap();
        let body: serde_json::Value = serde_json::from_str(&decoded.body).unwrap();
        assert_eq!(body["record_id"], "rec-1");
        assert!(body.get("local").is_none());
        assert_eq!(
            decoded.log_root,
            Some(PathBuf::from("/private/device/logs"))
        );

        let legacy = decode_forward_queue_item(r#"{"record_id":"legacy"}"#).unwrap();
        assert_eq!(legacy.body, r#"{"record_id":"legacy"}"#);
        assert!(legacy.log_root.is_none());

        assert!(decode_forward_queue_item(
            r#"{"tally_forward_queue_version":2,"record":{"record_id":"future"}}"#
        )
        .is_err());
    }

    #[test]
    fn private_uri_resolution_is_content_addressed_and_confined() {
        let root = Path::new("/safe/logs");
        let digest = "a".repeat(64);
        assert_eq!(
            resolve_private_uri(root, &format!("private://sha256/{digest}")),
            Some(
                root.join("private")
                    .join("objects")
                    .join("aa")
                    .join(format!("{digest}.json"))
            )
        );
        assert!(resolve_private_uri(root, "private://../../outside").is_none());
        assert!(resolve_private_uri(root, "private://sha256/not-a-digest").is_none());
    }

    #[test]
    fn full_removal_requires_and_accepts_tally_owned_paths() {
        let directory = test_directory("full-removal");
        let state_dir = directory.join("config/tally/logs/.state");
        let logs_dir = directory.join("custom-logs");
        fs::create_dir_all(state_dir.join("forward-queue")).unwrap();
        fs::write(state_dir.join("forward-queue/record.json"), b"{}\n").unwrap();
        fs::create_dir_all(&logs_dir).unwrap();
        fs::write(logs_dir.join("record.json"), b"{}\n").unwrap();

        let error = remove_tally_data(&state_dir, &logs_dir).unwrap_err();
        assert!(error.to_string().contains("unmarked log directory"));
        assert!(
            state_dir.exists(),
            "validation failure partially removed state"
        );
        assert!(
            logs_dir.exists(),
            "validation failure partially removed logs"
        );

        mark_tally_data_directory(&logs_dir).unwrap();
        remove_tally_data(&state_dir, &logs_dir).unwrap();
        assert!(!state_dir.exists());
        assert!(!logs_dir.exists());
        fs::remove_dir_all(directory).unwrap();
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
