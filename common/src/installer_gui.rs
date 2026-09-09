use crate::{
    install_options, InstallOptions, InstallReport, Result, UninstallReport, DEFAULT_API_URL,
};
use serde_json::{json, Value};
use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const INDEX: &str = include_str!("../installer-ui/index.html");
const STYLES: &str = include_str!("../installer-ui/style.css");
const APP: &str = include_str!("../installer-ui/app.js");
const OPENORIGINS_LOGO: &[u8] = include_bytes!("../installer-ui/oo-logo-horizontal.png");
const OPENORIGINS_ICON: &[u8] = include_bytes!("../../assets/oo-logo-no-text.png");
const MAX_BODY_BYTES: usize = 16 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub struct GuiClient {
    pub id: &'static str,
    pub product: &'static str,
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub installed_binary_path: PathBuf,
    pub available: bool,
    pub availability_detail: Option<String>,
    pub detected_version: Option<String>,
}

pub fn run_installer_gui<I, U, S>(
    clients: Vec<GuiClient>,
    mut install: I,
    mut uninstall: U,
    mut snapshot_paths: S,
) -> Result<()>
where
    I: FnMut(&str, InstallOptions) -> Result<InstallReport>,
    U: FnMut(&str, Option<PathBuf>, bool) -> Result<UninstallReport>,
    S: FnMut(&str, Option<&std::path::Path>) -> Result<Vec<PathBuf>>,
{
    if clients.is_empty() {
        return Err("installer requires at least one client".into());
    }
    let server = Server::http(("127.0.0.1", 0))
        .map_err(|error| format!("could not start local installer: {error}"))?;
    let address = server
        .server_addr()
        .to_ip()
        .ok_or("local installer did not receive an IP address")?;
    let origin = format!("http://{address}");
    let token = random_token()?;
    let url = format!("{origin}/#token={token}");

    if let Ok(path) = env::var("TALLY_GUI_URL_FILE") {
        super::atomic_write(PathBuf::from(path).as_path(), url.as_bytes(), 0o600)?;
    }
    println!("Tally installer: {url}");
    if env::var("TALLY_GUI_NO_OPEN").ok().as_deref() != Some("1") {
        open_browser(&url)?;
    }

    let mut last_activity = Instant::now();
    loop {
        let Some(request) = server.recv_timeout(Duration::from_secs(1))? else {
            if last_activity.elapsed() >= IDLE_TIMEOUT {
                return Ok(());
            }
            continue;
        };
        last_activity = Instant::now();
        let shutdown = handle_request(
            request,
            &origin,
            &token,
            &clients,
            &mut install,
            &mut uninstall,
            &mut snapshot_paths,
        );
        if shutdown {
            return Ok(());
        }
    }
}

fn handle_request<I, U, S>(
    mut request: Request,
    origin: &str,
    token: &str,
    clients: &[GuiClient],
    install: &mut I,
    uninstall: &mut U,
    snapshot_paths: &mut S,
) -> bool
where
    I: FnMut(&str, InstallOptions) -> Result<InstallReport>,
    U: FnMut(&str, Option<PathBuf>, bool) -> Result<UninstallReport>,
    S: FnMut(&str, Option<&std::path::Path>) -> Result<Vec<PathBuf>>,
{
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or(request.url());

    if method == Method::Get {
        let response = match path {
            "/" | "/index.html" => static_response(INDEX, "text/html; charset=utf-8"),
            "/style.css" => static_response(STYLES, "text/css; charset=utf-8"),
            "/app.js" => static_response(APP, "text/javascript; charset=utf-8"),
            "/oo-logo-horizontal.png" => static_bytes_response(OPENORIGINS_LOGO, "image/png"),
            "/oo-logo-no-text.png" => static_bytes_response(OPENORIGINS_ICON, "image/png"),
            _ => json_response(StatusCode(404), json!({"ok": false, "error": "Not found"})),
        };
        let _ = request.respond(response);
        return false;
    }

    if !authorized(&request, origin, token) {
        let _ = request.respond(json_response(
            StatusCode(403),
            json!({"ok": false, "error": "Installer request was not authorized"}),
        ));
        return false;
    }

    if method == Method::Post && path == "/api/status" {
        let client_status = clients
            .iter()
            .map(|client| {
                json!({
                    "id": client.id,
                    "product": client.product,
                    "configPath": client.config_path,
                    "keyPath": crate::api_key_path(&client.state_dir),
                    "installed": client.installed_binary_path.exists()
                        && crate::api_key_path(&client.state_dir).exists(),
                    "available": client.available,
                    "availabilityDetail": client.availability_detail,
                    "detectedVersion": client.detected_version,
                })
            })
            .collect::<Vec<_>>();
        let response = json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "clients": client_status,
            "defaultApiUrl": DEFAULT_API_URL,
        });
        let _ = request.respond(json_response(StatusCode(200), response));
        return false;
    }

    if method == Method::Post && path == "/api/shutdown" {
        let _ = request.respond(json_response(StatusCode(200), json!({"ok": true})));
        return true;
    }

    if method == Method::Post && path == "/api/uninstall" {
        let (selected, remove_data) = match read_json_body(&mut request).and_then(|value| {
            let remove_data = value
                .get("removeData")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            parse_client_requests(value, clients).map(|selected| (selected, remove_data))
        }) {
            Ok(parsed) => parsed,
            Err(error) => {
                let _ = request.respond(json_response(
                    StatusCode(400),
                    json!({"ok": false, "error": error.to_string()}),
                ));
                return false;
            }
        };
        let result = selected
            .iter()
            .map(|selection| uninstall(selection.id, selection.config_path.clone(), remove_data))
            .collect::<Result<Vec<_>>>();
        let (response, shutdown_after_response) = match result {
            Ok(reports) => {
                let details = reports
                    .iter()
                    .zip(selected.iter())
                    .map(|(report, selection)| {
                        json!({
                            "id": selection.id,
                            "configPath": report.config_path,
                            "journalPath": report.journal_path,
                            "logsPath": report.logs_path,
                            "dataRemoved": report.data_removed,
                        })
                    })
                    .collect::<Vec<_>>();
                (
                    json_response(
                        StatusCode(200),
                        json!({
                            "ok": true,
                            "dataRemoved": remove_data,
                            "clients": details,
                        }),
                    ),
                    true,
                )
            }
            Err(error) => (
                json_response(
                    StatusCode(400),
                    json!({"ok": false, "error": error.to_string()}),
                ),
                false,
            ),
        };
        let _ = request.respond(response);
        return shutdown_after_response;
    }

    if method == Method::Post && path == "/api/install" {
        let result = (|| -> Result<Vec<(&str, InstallReport)>> {
            let parsed = parse_install_request(read_json_body(&mut request)?, clients)?;
            install_selected_clients(parsed, install, snapshot_paths)
        })();
        let (response, shutdown_after_response) = match result {
            Ok(reports) => {
                let connected = reports
                    .iter()
                    .all(|(_, report)| report.handshake_error.is_none());
                let details = reports
                    .iter()
                    .map(|(id, report)| {
                        json!({
                            "id": id,
                            "configPath": report.config_path,
                            "keyPath": crate::api_key_path(&report.state_dir),
                            "logsPath": report.logs_path,
                            "installedBinaryPath": report.installed_binary_path,
                            "backupPath": report.backup_path,
                            "connected": report.handshake_error.is_none(),
                            "approvalRequired": report.approval_required,
                            "approvalInstructions": report.approval_instructions,
                            "clientVersion": report.client_version,
                        })
                    })
                    .collect::<Vec<_>>();
                (
                    json_response(
                        StatusCode(200),
                        json!({
                            "ok": true,
                            "connected": connected,
                            "approvalRequired": reports.iter().any(|(_, report)| report.approval_required),
                            "warning": (!connected).then_some("The dashboard could not confirm every selected client automatically. Local logging is installed and will continue offline. Try the key again, or use \"Mark connected manually\" in the dashboard if needed."),
                            "clients": details,
                        }),
                    ),
                    connected,
                )
            }
            Err(error) => (
                json_response(
                    StatusCode(400),
                    json!({"ok": false, "error": error.to_string()}),
                ),
                false,
            ),
        };
        let _ = request.respond(response);
        return shutdown_after_response;
    }

    let _ = request.respond(json_response(
        StatusCode(404),
        json!({"ok": false, "error": "Not found"}),
    ));
    false
}

fn install_selected_clients<I, S>(
    parsed: InstallRequest,
    install: &mut I,
    snapshot_paths: &mut S,
) -> Result<Vec<(&'static str, InstallReport)>>
where
    I: FnMut(&str, InstallOptions) -> Result<InstallReport>,
    S: FnMut(&str, Option<&std::path::Path>) -> Result<Vec<PathBuf>>,
{
    let snapshots = parsed
        .clients
        .iter()
        .map(|selection| snapshot_paths(selection.id, selection.config_path.as_deref()))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .map(|path| crate::FileSnapshot::capture(&path))
        .collect::<std::io::Result<Vec<_>>>()?;
    let install_result = parsed
        .clients
        .iter()
        .map(|selection| {
            let options = install_options(
                parsed.api_key.clone(),
                parsed.api_url.clone(),
                selection
                    .config_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            )?;
            install(selection.id, options).map(|report| (selection.id, report))
        })
        .collect::<Result<Vec<_>>>();
    match install_result {
        Ok(reports) => Ok(reports),
        Err(error) => {
            let snapshots = snapshots.iter().collect::<Vec<_>>();
            Err(crate::install_error_with_rollback(error, &snapshots))
        }
    }
}

struct InstallRequest {
    api_key: String,
    api_url: Option<String>,
    clients: Vec<ClientRequest>,
}

struct ClientRequest {
    id: &'static str,
    config_path: Option<PathBuf>,
}

fn parse_install_request(value: Value, clients: &[GuiClient]) -> Result<InstallRequest> {
    let object = value
        .as_object()
        .ok_or("request body must be a JSON object")?;
    let api_key = object
        .get("apiKey")
        .and_then(Value::as_str)
        .ok_or("Agent API key is required")?
        .to_string();
    let api_url = object
        .get("apiUrl")
        .and_then(Value::as_str)
        .map(str::to_string);
    let selected = parse_client_requests(value, clients)?;
    Ok(InstallRequest {
        api_key,
        api_url,
        clients: selected,
    })
}

fn parse_client_requests(value: Value, clients: &[GuiClient]) -> Result<Vec<ClientRequest>> {
    let object = value
        .as_object()
        .ok_or("request body must be a JSON object")?;
    let requested = object
        .get("clients")
        .and_then(Value::as_array)
        .ok_or("choose at least one client")?;
    if requested.is_empty() {
        return Err("choose at least one client".into());
    }
    let mut selected = Vec::new();
    for request in requested {
        let request = request.as_object().ok_or("invalid client selection")?;
        let id = request
            .get("id")
            .and_then(Value::as_str)
            .ok_or("client id is required")?;
        if selected
            .iter()
            .any(|selection: &ClientRequest| selection.id == id)
        {
            return Err(format!("client {id} was selected more than once").into());
        }
        let client = clients
            .iter()
            .find(|client| client.id == id)
            .ok_or_else(|| format!("unknown client: {id}"))?;
        let config_path = request
            .get("configPath")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        selected.push(ClientRequest {
            id: client.id,
            config_path,
        });
    }
    Ok(selected)
}

fn read_json_body(request: &mut Request) -> Result<Value> {
    if request.body_length().unwrap_or(0) > MAX_BODY_BYTES {
        return Err("request body is too large".into());
    }
    let mut body = Vec::new();
    request
        .as_reader()
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > MAX_BODY_BYTES {
        return Err("request body is too large".into());
    }
    Ok(serde_json::from_slice(&body)?)
}

fn authorized(request: &Request, origin: &str, token: &str) -> bool {
    let header = |name: &str| {
        request
            .headers()
            .iter()
            .find(|header| header.field.to_string().eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    };
    let token_matches = header("x-tally-installer-token")
        .is_some_and(|value| constant_time_eq(value.as_bytes(), token.as_bytes()));
    let origin_matches = header("origin").is_none_or(|value| value == origin);
    token_matches && origin_matches
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn random_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("could not create installer token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn static_response(
    body: &'static str,
    content_type: &'static str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    secured(Response::from_string(body).with_header(header("content-type", content_type)))
}

fn static_bytes_response(
    body: &'static [u8],
    content_type: &'static str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    secured(Response::from_data(body).with_header(header("content-type", content_type)))
}

fn json_response(status: StatusCode, body: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    secured(
        Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(header("content-type", "application/json; charset=utf-8")),
    )
}

fn secured<T: Read + Send + 'static>(response: Response<T>) -> Response<T> {
    response
        .with_header(header("cache-control", "no-store"))
        .with_header(header("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"))
        .with_header(header("referrer-policy", "no-referrer"))
        .with_header(header("x-content-type-options", "nosniff"))
        .with_header(header("x-frame-options", "DENY"))
}

fn header(name: &'static str, value: &'static str) -> Header {
    Header::from_bytes(name, value).expect("valid static HTTP header")
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(not(any(unix, target_os = "windows")))]
    return Err("opening a browser is not supported on this platform".into());

    command
        .spawn()
        .map_err(|error| -> Box<dyn std::error::Error> {
            format!("could not open the installer in a browser: {error}").into()
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{install_selected_clients, ClientRequest, InstallRequest};
    use crate::InstallReport;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn multi_client_install_rolls_back_every_selected_client() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tally-installer-transaction-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let first = root.join("first.json");
        let second = root.join("second.json");
        fs::write(&first, b"first-before").unwrap();
        fs::write(&second, b"second-before").unwrap();
        let parsed = InstallRequest {
            api_key: "test-key".to_string(),
            api_url: Some("http://127.0.0.1:8080/v1/tally/logs".to_string()),
            clients: vec![
                ClientRequest {
                    id: "first",
                    config_path: Some(first.clone()),
                },
                ClientRequest {
                    id: "second",
                    config_path: Some(second.clone()),
                },
            ],
        };
        let mut snapshot_paths =
            |_: &str, path: Option<&std::path::Path>| Ok(vec![path.unwrap().to_path_buf()]);
        let mut install = |id: &str, options: crate::InstallOptions| {
            let path = options.config_path.unwrap();
            fs::write(&path, format!("{id}-after"))?;
            if id == "second" {
                return Err("second client failed".into());
            }
            Ok(InstallReport {
                config_path: path,
                state_dir: PathBuf::new(),
                logs_path: PathBuf::new(),
                installed_binary_path: PathBuf::new(),
                backup_path: None,
                handshake_error: None,
                approval_required: false,
                approval_instructions: None,
                client_version: None,
            })
        };

        let error = install_selected_clients(parsed, &mut install, &mut snapshot_paths)
            .unwrap_err()
            .to_string();
        assert!(error.contains("second client failed"));
        assert_eq!(fs::read(&first).unwrap(), b"first-before");
        assert_eq!(fs::read(&second).unwrap(), b"second-before");
        fs::remove_dir_all(root).unwrap();
    }
}
