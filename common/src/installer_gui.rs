use crate::{install_options, InstallOptions, InstallReport, Result, DEFAULT_API_URL};
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
const MAX_BODY_BYTES: usize = 16 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub struct GuiConfig {
    pub product: &'static str,
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub installed_binary_path: PathBuf,
}

pub fn run_installer_gui<F>(config: GuiConfig, mut install: F) -> Result<()>
where
    F: FnMut(InstallOptions) -> Result<InstallReport>,
{
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
        let shutdown = handle_request(request, &origin, &token, &config, &mut install);
        if shutdown {
            return Ok(());
        }
    }
}

fn handle_request<F>(
    mut request: Request,
    origin: &str,
    token: &str,
    config: &GuiConfig,
    install: &mut F,
) -> bool
where
    F: FnMut(InstallOptions) -> Result<InstallReport>,
{
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or(request.url());

    if method == Method::Get {
        let response = match path {
            "/" | "/index.html" => static_response(INDEX, "text/html; charset=utf-8"),
            "/style.css" => static_response(STYLES, "text/css; charset=utf-8"),
            "/app.js" => static_response(APP, "text/javascript; charset=utf-8"),
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
        let installed = config.installed_binary_path.exists()
            && crate::api_key_path(&config.state_dir).exists();
        let response = json!({
            "ok": true,
            "product": config.product,
            "configPath": config.config_path,
            "keyPath": crate::api_key_path(&config.state_dir),
            "defaultApiUrl": DEFAULT_API_URL,
            "installed": installed,
        });
        let _ = request.respond(json_response(StatusCode(200), response));
        return false;
    }

    if method == Method::Post && path == "/api/shutdown" {
        let _ = request.respond(json_response(StatusCode(200), json!({"ok": true})));
        return true;
    }

    if method == Method::Post && path == "/api/install" {
        let (response, shutdown_after_response) = match read_json_body(&mut request)
            .and_then(parse_options)
            .and_then(install)
        {
            Ok(report) => {
                let connected = report.handshake_error.is_none();
                (
                    json_response(
                        StatusCode(200),
                        json!({
                            "ok": true,
                            "connected": connected,
                            "warning": report.handshake_error.as_ref().map(|_| "The dashboard could not confirm this client automatically. Local logging is installed and will continue offline. Use \"Mark connected manually\" in the dashboard if needed."),
                            "configPath": report.config_path,
                            "keyPath": crate::api_key_path(&report.state_dir),
                            "logsPath": report.logs_path,
                            "installedBinaryPath": report.installed_binary_path,
                            "backupPath": report.backup_path,
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

fn parse_options(value: Value) -> Result<InstallOptions> {
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
    let config_path = object
        .get("configPath")
        .and_then(Value::as_str)
        .map(str::to_string);
    install_options(api_key, api_url, config_path)
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
        .with_header(header("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"))
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
