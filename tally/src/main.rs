#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::env;
use std::path::PathBuf;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() {
    if let Err(error) = run() {
        eprintln!("Tally: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("gui") => run_gui(),
        Some(argument) if argument.starts_with("-psn_") => run_gui(),
        Some("codex") => run_client(tally_codex::dispatch(args.collect())),
        Some("claude") => run_client(tally_claude::dispatch(args.collect())),
        Some("forward-pending") => {
            let state_dir = env::var("TALLY_STATE_DIR")
                .map(PathBuf::from)
                .map_err(|_| "TALLY_STATE_DIR is required for forwarding")?;
            tally_common::forward_pending(&state_dir)
        }
        Some("heartbeat-daemon" | "daemon") => {
            let client = client_from_environment()?;
            run_client(match client {
                "codex" => tally_codex::dispatch(vec!["heartbeat-daemon".to_string()]),
                _ => tally_claude::dispatch(vec!["heartbeat-daemon".to_string()]),
            })
        }
        Some("--version" | "version") => {
            println!("Tally {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help" | "-h" | "help") => {
            println!(
                "Tally {}\n\nOpen Tally without arguments to install or update integrations.",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
        Some(other) => Err(format!("unknown internal command: {other}").into()),
    }
}

fn run_gui() -> Result<()> {
    let codex_cli = tally_codex::codex_cli_status();
    let (codex_available, codex_detail, codex_version) = match codex_cli {
        Ok(status) => (
            true,
            Some(format!(
                "{} at {}",
                status.version,
                status.command.display()
            )),
            Some(status.version),
        ),
        Err(_) => (
            false,
            Some(
                "Codex CLI is required for Codex Desktop. Install or update it, confirm `codex --version` works, then reopen Tally."
                    .to_string(),
            ),
            None,
        ),
    };
    tally_common::run_installer_gui(
        vec![
            tally_common::GuiClient {
                id: "codex",
                product: "Codex",
                config_path: tally_codex::default_config_path(),
                state_dir: tally_codex::default_state_dir(),
                installed_binary_path: tally_codex::default_installed_binary_path(),
                available: codex_available,
                availability_detail: codex_detail,
                detected_version: codex_version,
            },
            tally_common::GuiClient {
                id: "claude",
                product: "Claude Code",
                config_path: tally_claude::default_config_path(),
                state_dir: tally_claude::default_state_dir(),
                installed_binary_path: tally_claude::default_installed_binary_path(),
                available: true,
                availability_detail: None,
                detected_version: None,
            },
        ],
        |client, options| match client {
            "codex" => tally_codex::install_desktop_hooks(options),
            "claude" => tally_claude::install_desktop_hooks(options),
            _ => Err(format!("unknown client: {client}").into()),
        },
        |client, config_path, remove_data| match client {
            "codex" => tally_codex::uninstall_desktop_hooks_with_options(config_path, remove_data),
            "claude" => {
                tally_claude::uninstall_desktop_hooks_with_options(config_path, remove_data)
            }
            _ => Err(format!("unknown client: {client}").into()),
        },
        |client, config_path| match client {
            "codex" => Ok(tally_codex::installation_snapshot_paths(config_path)),
            "claude" => Ok(tally_claude::installation_snapshot_paths(config_path)),
            _ => Err(format!("unknown client: {client}").into()),
        },
    )
}

fn client_from_environment() -> Result<&'static str> {
    let agent = env::var("TALLY_AGENT_ID").unwrap_or_default();
    if agent.contains("codex") {
        Ok("codex")
    } else if agent.contains("claude") {
        Ok("claude")
    } else {
        Err("could not identify the hook client".into())
    }
}

fn run_client(result: tally_common::Result<i32>) -> Result<()> {
    let code = result?;
    if code == 0 {
        Ok(())
    } else {
        Err(format!("client command exited with code {code}").into())
    }
}
