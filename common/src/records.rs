use crate::agent_runtime::{
    env_enabled, evidence_summary, first_mapping_by_key, first_string_by_key,
    first_value_by_key, server_evidence, stable_id, AuditSink,
};
use crate::Result;
use serde_json::{json, Value};

pub struct HookRecordProfile {
    pub hook_field: &'static str,
    pub lifecycle_record_type: &'static str,
    pub default_tool_server: &'static str,
    pub prompt_summary_label: &'static str,
    pub result_summary_label: &'static str,
    pub tool_param_keys: &'static [&'static str],
    pub instruction_id_keys: &'static [&'static str],
    pub tool_server_keys: &'static [&'static str],
    pub tool_name_keys: &'static [&'static str],
    pub error_keys: &'static [&'static str],
}

pub struct HookRecordIdentity<'a> {
    pub session_id: &'a str,
    pub agent_id: &'a str,
    pub agent_version: &'a str,
    pub action_id: &'a str,
    pub turn_id: &'a str,
}

pub fn build_hook_record(
    sink: &AuditSink,
    profile: &HookRecordProfile,
    identity: &HookRecordIdentity<'_>,
    event_type: &str,
    payload: &Value,
    raw_ref: &Value,
    metadata: &Value,
) -> Result<Value> {
    let prompt = first_string_by_key(
        payload,
        &["prompt", "user_prompt", "input", "text", "content"],
    );
    let raw_hash = raw_ref["hash"].clone();
    let raw_uri = raw_ref["uri"].clone();
    let observed_at = metadata["observed_at"].clone();

    let record = match event_type {
        "SessionStart" => json!({
            "record_type": "SESSION_START",
            "schema_version": "0.2",
            "session_id": identity.session_id,
            "agent_id": identity.agent_id,
            "agent_version": identity.agent_version,
            "principal": {"type": Value::Null, "id": Value::Null, "capture_status": "unavailable"},
            "authority_scope_hash": Value::Null,
            "authority_scope_uri": Value::Null,
            "authority_capture_status": "unavailable",
            "authority_granted_at": Value::Null,
            "session_started_at": metadata["observed_at"],
            "workspace": workspace_summary(&metadata["git_state"]),
            "raw_hook_hash": raw_hash,
            "raw_hook_uri": raw_uri,
        }),
        "UserPromptSubmit" => {
            let evidence =
                server_evidence(&prompt.map(Value::String).unwrap_or_else(|| payload.clone()));
            let summary = evidence_summary(&evidence, profile.prompt_summary_label);
            let context_ref = sink.private_payload(&metadata["git_state"])?;
            json!({
                "record_type": "INSTRUCTION_RECEIVED",
                "schema_version": "0.2",
                "session_id": identity.session_id,
                "instruction_id": stable_id("instr", payload),
                "sender": {"id": Value::Null, "signature": Value::Null, "signature_status": "unavailable"},
                "instruction_hash": raw_hash,
                "instruction_uri": raw_uri,
                "instruction_received_at": observed_at,
                "context_snapshot_hash": context_ref["hash"],
                "context_snapshot_uri": context_ref["uri"],
                "workspace": workspace_summary(&metadata["git_state"]),
                "declared_intent": {
                    "summary": Value::Null,
                    "detail_hash": Value::Null,
                    "detail_uri": Value::Null,
                    "capture_status": "unavailable",
                },
                "instruction_summary": format!("[ARB] {summary}"),
                "server_evidence": evidence,
            })
        }
        "PreToolUse" | "PermissionRequest" => {
            let tool_params = first_mapping_by_key(payload, profile.tool_param_keys)
                .cloned()
                .unwrap_or_else(|| payload.clone());
            let evidence = server_evidence(&tool_params);
            let params_ref = sink.private_payload(&tool_params)?;
            let pre_state_ref = sink.private_payload(&metadata["git_state"])?;
            json!({
                "record_type": "ACTION_TAKEN",
                "schema_version": "0.2",
                "session_id": identity.session_id,
                "action_id": identity.action_id,
                "instruction_id": first_string_by_key(payload, profile.instruction_id_keys)
                    .unwrap_or_else(|| stable_id("instr", &Value::String(identity.session_id.to_string()))),
                "action_type": if event_type == "PermissionRequest" { "decision" } else { "tool_call" },
                "tool": {
                    "server": first_string_by_key(payload, profile.tool_server_keys)
                        .unwrap_or_else(|| profile.default_tool_server.to_string()),
                    "name": first_string_by_key(payload, profile.tool_name_keys)
                        .unwrap_or_else(|| event_type.to_string()),
                    "params_hash": params_ref["hash"],
                    "params_uri": params_ref["uri"],
                },
                "pre_state_hash": pre_state_ref["hash"],
                "pre_state_uri": pre_state_ref["uri"],
                "post_state_hash": Value::Null,
                "post_state_uri": Value::Null,
                "action_timestamp": observed_at,
                "deviance_flag": {"deviated": Value::Null, "evaluation_status": "unavailable", "delta_category": Value::Null, "delta_hash": Value::Null, "delta_uri": Value::Null},
                "server_evidence": evidence,
                "raw_hook_hash": raw_ref["hash"],
                "raw_hook_uri": raw_ref["uri"],
            })
        }
        "PostToolUse" => {
            let has_error = first_string_by_key(payload, profile.error_keys).is_some();
            let evidence_source = first_value_by_key(
                payload,
                &[
                    "tool_response",
                    "tool_result",
                    "result",
                    "output",
                    "content",
                ],
            )
            .unwrap_or(payload);
            let evidence = server_evidence(evidence_source);
            let summary = evidence_summary(&evidence, profile.result_summary_label);
            let post_state_ref = sink.private_payload(&metadata["git_state"])?;
            json!({
                "record_type": "RESULT_RECEIVED",
                "schema_version": "0.2",
                "session_id": identity.session_id,
                "action_id": identity.action_id,
                "result_hash": raw_ref["hash"],
                "result_uri": raw_ref["uri"],
                "result_received_at": observed_at,
                "post_state_hash": post_state_ref["hash"],
                "post_state_uri": post_state_ref["uri"],
                "result_interpretation": {
                    "summary": format!("[ARB] {summary}"),
                    "detail_hash": raw_ref["hash"],
                    "detail_uri": raw_ref["uri"],
                },
                "exception": {
                    "occurred": has_error,
                    "type": first_string_by_key(payload, &["error_type", "type"]),
                    "description_hash": if has_error { raw_ref["hash"].clone() } else { Value::Null },
                    "description_uri": if has_error { raw_ref["uri"].clone() } else { Value::Null },
                },
                "server_evidence": evidence,
            })
        }
        "Stop" => {
            let evidence_source = first_value_by_key(
                payload,
                &[
                    "last_assistant_message",
                    "response",
                    "result",
                    "output",
                    "content",
                ],
            )
            .unwrap_or(payload);
            json!({
                "record_type": "TURN_END",
                "schema_version": "0.2",
                "session_id": identity.session_id,
                "turn_id": identity.turn_id,
                "outcome": "completed",
                "outcome_hash": raw_ref["hash"],
                "outcome_uri": raw_ref["uri"],
                "turn_ended_at": observed_at,
                "server_evidence": server_evidence(evidence_source),
            })
        }
        "SessionEnd" => json!({
            "record_type": "SESSION_END",
            "schema_version": "0.2",
            "session_id": identity.session_id,
            "outcome": Value::Null,
            "outcome_capture_status": "unavailable",
            "outcome_hash": raw_ref["hash"],
            "outcome_uri": raw_ref["uri"],
            "session_ended_at": observed_at,
            "session_end_reason": first_string_by_key(payload, &["reason"]),
        }),
        "SubagentStart" | "SubagentStop" => {
            let receiver_id = first_string_by_key(
                payload,
                &["subagent_id", "subagentId", "agent_name", "agent_type"],
            );
            let handoff_id = stable_id(
                "handoff",
                &json!({"session_id": identity.session_id, "receiver_id": receiver_id}),
            );
            json!({
                "record_type": "HANDOFF",
                "schema_version": "0.2",
                "session_id": identity.session_id,
                "handoff_id": handoff_id,
                "emitting_party": "sender",
                "sender": {"agent_id": identity.agent_id, "org_id": Value::Null, "signature": Value::Null, "signature_status": "unavailable"},
                "receiver": {"agent_id": receiver_id, "org_id": Value::Null, "signature": Value::Null, "acknowledged_at": if event_type == "SubagentStop" { observed_at.clone() } else { Value::Null }},
                "payload_hash": raw_hash,
                "payload_uri": raw_uri,
                "handoff_timestamp": observed_at,
                "acknowledgement_status": if event_type == "SubagentStop" { "acknowledged" } else { "pending" },
            })
        }
        _ => json!({
            "record_type": profile.lifecycle_record_type,
            "schema_version": "0.2",
            "session_id": identity.session_id,
            "event_hash": raw_ref["hash"],
            "event_uri": raw_ref["uri"],
            "observed_at": observed_at,
            "metadata": metadata,
        }),
    };
    Ok(with_hook_event(record, profile.hook_field, event_type))
}


pub fn workspace_summary(git_state: &Value) -> Value {
    if !env_enabled("TALLY_WORKSPACE_DETAILS_ENABLED", true) {
        return Value::Null;
    }
    let Some(path) = git_state["workspace"].as_str().filter(|p| !p.is_empty()) else {
        return Value::Null;
    };
    let field = |key: &str, placeholder: &str| {
        git_state[key]
            .as_str()
            .filter(|v| !v.is_empty() && *v != placeholder)
            .map(|v| Value::String(v.to_string()))
            .unwrap_or(Value::Null)
    };
    json!({
        "path": path,
        "name": std::path::Path::new(path).file_name().and_then(|n| n.to_str()),
        "is_git_repo": git_state["is_git_repo"].as_bool().unwrap_or(false),
        "branch": field("branch", "(detached)"),
        "head": field("head", "(initial)"),
    })
}

fn with_hook_event(mut record: Value, field: &str, event_type: &str) -> Value {
    record[field] = Value::String(event_type.to_string());
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_summary_exposes_identity_not_status() {
        let git_state = json!({
            "is_git_repo": true,
            "workspace": "/home/u/origo-backend",
            "head": "a774ef8",
            "branch": "cpu-rule-compiler",
            "status": "# branch.oid a774ef8\n1 .M N... src/secret.rs",
            "status_hash": "sha256:00",
        });
        assert_eq!(
            workspace_summary(&git_state),
            json!({
                "path": "/home/u/origo-backend",
                "name": "origo-backend",
                "is_git_repo": true,
                "branch": "cpu-rule-compiler",
                "head": "a774ef8",
            })
        );
    }

    #[test]
    fn workspace_summary_drops_git_placeholders() {
        let git_state = json!({
            "is_git_repo": true,
            "workspace": "/r",
            "head": "(initial)",
            "branch": "(detached)",
        });
        let summary = workspace_summary(&git_state);
        assert!(summary["branch"].is_null());
        assert!(summary["head"].is_null());
    }

    #[test]
    fn workspace_summary_outside_git() {
        let summary = workspace_summary(&json!({"is_git_repo": false, "workspace": "/tmp/x"}));
        assert_eq!(summary["path"], "/tmp/x");
        assert_eq!(summary["is_git_repo"], false);
        assert!(summary["branch"].is_null());
        assert!(workspace_summary(&Value::Null).is_null());
    }
}
