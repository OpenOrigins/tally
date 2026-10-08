//! Local workspace exclusions applied before capture and before delivery.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use url::Url;

use crate::Result;

const POLICY_FILE: &str = ".config/tally/privacy.json";

#[derive(Default)]
pub struct PrivacyPolicy {
    owners: Vec<String>,
    paths: Vec<PathBuf>,
}

impl PrivacyPolicy {
    pub fn load() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or("home directory is unavailable; cannot load privacy policy")?;
        let path = Path::new(&home).join(POLICY_FILE);
        match fs::read_to_string(path) {
            Ok(contents) => Self::parse(&contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn parse(contents: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(contents)?;
        if !value.is_object() {
            return Err("privacy policy must be a JSON object".into());
        }
        let strings = |key: &str| -> Result<Vec<String>> {
            match value.get(key) {
                None => Ok(Vec::new()),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .filter(|text| !text.trim().is_empty())
                            .map(str::to_owned)
                            .ok_or_else(|| {
                                format!("privacy policy {key} must contain nonempty strings").into()
                            })
                    })
                    .collect(),
                _ => Err(format!("privacy policy {key} must be an array").into()),
            }
        };
        let paths = strings("excluded_paths")?
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if paths.iter().any(|path| !path.is_absolute()) {
            return Err("privacy policy excluded_paths must be absolute".into());
        }
        Ok(Self {
            owners: strings("excluded_git_owners")?
                .into_iter()
                .map(|owner| owner.to_ascii_lowercase())
                .collect(),
            paths,
        })
    }

    pub fn active(&self) -> bool {
        !self.owners.is_empty() || !self.paths.is_empty()
    }

    pub fn excludes_workspace(&self, workspace: &Path) -> Result<bool> {
        if !self.active() {
            return Ok(false);
        }
        let workspace = fs::canonicalize(workspace)?;
        for path in &self.paths {
            if workspace.starts_with(fs::canonicalize(path)?) {
                return Ok(true);
            }
        }
        if self.owners.is_empty() {
            return Ok(false);
        }
        let git_directory = if workspace.is_file() {
            workspace.parent().ok_or("workspace file has no parent")?
        } else {
            &workspace
        };
        let root = git_output(git_directory, &["rev-parse", "--show-toplevel"])?;
        let Some(root) = root else {
            return Err("workspace is not a classifiable Git repository".into());
        };
        let root = PathBuf::from(root.trim());
        let remotes = git_output(&root, &["remote"])?.ok_or("could not inspect Git remotes")?;
        if remotes.trim().is_empty() {
            return Err("Git repository has no remote; privacy classification is uncertain".into());
        }
        for remote in remotes.lines() {
            let urls = git_output(&root, &["remote", "get-url", "--all", remote])?
                .ok_or("could not inspect Git remote URL")?;
            if urls.trim().is_empty() {
                return Err("Git remote has no URL; privacy classification is uncertain".into());
            }
            for url in urls.lines() {
                let owner = remote_owner(url)
                    .ok_or("Git remote owner cannot be classified by local privacy policy")?;
                if self.owners.iter().any(|blocked| blocked == &owner) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn excludes_context(&self, workspace: &Path, payload: &Value) -> Result<bool> {
        if self.excludes_workspace(workspace)? {
            return Ok(true);
        }
        let mut candidates = Vec::new();
        collect_paths(payload, &mut candidates);
        for candidate in candidates {
            if self.excludes_workspace(&candidate)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub fn capture_blocked(workspace: &Path, payload: &Value) -> bool {
    PrivacyPolicy::load()
        .and_then(|policy| {
            if policy.excludes_context(workspace, payload)? {
                return Ok(true);
            }
            policy.excludes_workspace(&std::env::current_dir()?)
        })
        .unwrap_or(true)
}

pub fn delivery_blocked(record: &Value) -> bool {
    let Ok(policy) = PrivacyPolicy::load() else {
        return true;
    };
    if !policy.active() {
        return false;
    }
    let Some(workspace) = record.get("workspace").and_then(Value::as_str) else {
        return true;
    };
    policy
        .excludes_context(Path::new(workspace), record)
        .unwrap_or(true)
}

fn collect_paths(value: &Value, paths: &mut Vec<PathBuf>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "cwd"
                        | "workspace"
                        | "workspace_root"
                        | "workspace_roots"
                        | "project_root"
                        | "repository_path"
                        | "file_path"
                ) || key.ends_with("_path")
                {
                    if let Some(path) = value.as_str().filter(|path| Path::new(path).is_absolute())
                    {
                        paths.push(PathBuf::from(path));
                    }
                    if let Some(values) = value.as_array() {
                        paths.extend(
                            values
                                .iter()
                                .filter_map(Value::as_str)
                                .filter(|path| Path::new(path).is_absolute())
                                .map(PathBuf::from),
                        );
                    }
                }
                collect_paths(value, paths);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_paths(item, paths)),
        _ => {}
    }
}

fn git_output(workspace: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8(output.stdout)?))
}

fn remote_owner(remote: &str) -> Option<String> {
    let path = if let Ok(url) = Url::parse(remote) {
        if !url.host_str()?.eq_ignore_ascii_case("github.com") {
            return None;
        }
        url.path().trim_start_matches('/').to_owned()
    } else {
        let (host, path) = remote.split_once(':')?;
        if !host.rsplit('@').next()?.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path.to_owned()
    };
    let mut parts = path.split('/');
    let owner = parts.next()?;
    if owner.is_empty() || parts.next().is_none() {
        return None;
    }
    Some(owner.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_remote_owner_formats() {
        assert_eq!(
            remote_owner("git@github.com:Metal-Minds-LLC/repo.git"),
            Some("metal-minds-llc".into())
        );
        assert_eq!(
            remote_owner("https://github.com/Metal-Minds-LLC/repo.git"),
            Some("metal-minds-llc".into())
        );
        assert_eq!(
            remote_owner("https://example.com/Metal-Minds-LLC/repo"),
            None
        );
    }

    #[test]
    fn known_paths_and_payload_paths_are_excluded() {
        let root = std::env::temp_dir().join(format!("tally-privacy-{}", std::process::id()));
        let blocked = root.join("blocked");
        let allowed = root.join("allowed");
        fs::create_dir_all(&blocked).unwrap();
        fs::create_dir_all(&allowed).unwrap();
        let policy = PrivacyPolicy {
            owners: vec![],
            paths: vec![blocked.clone()],
        };
        assert!(policy.excludes_workspace(&blocked).unwrap());
        assert!(policy
            .excludes_context(&allowed, &serde_json::json!({"cwd": blocked}))
            .unwrap());
        assert!(!policy.excludes_workspace(&allowed).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn github_owner_is_excluded_for_nested_workspace() {
        let root = std::env::temp_dir().join(format!(
            "tally-remote-{}-{}",
            std::process::id(),
            crate::agent_runtime::unix_now_millis()
        ));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        assert!(Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&root)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "remote",
                "add",
                "origin",
                "git@github.com:Metal-Minds-LLC/future-repo.git"
            ])
            .status()
            .unwrap()
            .success());
        let policy = PrivacyPolicy {
            owners: vec!["metal-minds-llc".into()],
            paths: vec![],
        };
        assert!(policy.excludes_workspace(&nested).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unclassifiable_remote_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "tally-unknown-remote-{}-{}",
            std::process::id(),
            crate::agent_runtime::unix_now_millis()
        ));
        fs::create_dir_all(&root).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "remote",
                "add",
                "origin",
                "https://example.com/team/repo.git"
            ])
            .status()
            .unwrap()
            .success());
        let policy = PrivacyPolicy {
            owners: vec!["metal-minds-llc".into()],
            paths: vec![],
        };
        assert!(policy.excludes_workspace(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
