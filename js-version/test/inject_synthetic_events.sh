#!/usr/bin/env bash
# Five of the ten requested events don't reliably fire from a single
# non-interactive `claude --print` call against a dummy API, for reasons
# confirmed by decompiling the actual claude-code binary (not guessed):
#
#   PermissionRequest - the binary's own permission-request path is skipped
#                        entirely in --print/non-interactive mode (there is
#                        no one to ask); PreToolUse is what fires instead.
#   PreCompact/        - only fire on an actual context-window compaction
#   PostCompact          (manual /compact or auto-overflow), which a single
#                        short dummy turn never reaches.
#   SubagentStart/     - only fire when the model actually invokes the Task
#   SubagentStop         tool to spawn a subagent, which requires orchestrating
#                        a second nested mock conversation reliably.
#
# So this script exercises the *exact same* hooks/log_hook.sh that
# .claude/settings.json wires up for real, but feeds it hand-built payloads
# that match the verified request schemas pulled from the shipped binary
# (see README.md for the schema source). This proves the logging path works
# end-to-end for these events even though we can't organically trigger them
# in one dummy run.
set -uo pipefail

HOOK="/workspace/hooks/log_hook.sh"
SESSION_ID="sess-synthetic-0001"
CWD="/workspace"

emit() {
  echo "$1" | bash "$HOOK"
}

echo "[inject_synthetic_events] PermissionRequest"
emit "$(jq -nc --arg sid "$SESSION_ID" --arg cwd "$CWD" '{
  session_id: $sid,
  transcript_path: "/workspace/.claude/transcript-synthetic.jsonl",
  cwd: $cwd,
  hook_event_name: "PermissionRequest",
  tool_name: "Bash",
  tool_input: { command: "rm -rf /tmp/synthetic-demo" }
}')"

echo "[inject_synthetic_events] PreCompact"
emit "$(jq -nc --arg sid "$SESSION_ID" --arg cwd "$CWD" '{
  session_id: $sid,
  transcript_path: "/workspace/.claude/transcript-synthetic.jsonl",
  cwd: $cwd,
  hook_event_name: "PreCompact",
  trigger: "manual",
  custom_instructions: null
}')"

echo "[inject_synthetic_events] PostCompact"
emit "$(jq -nc --arg sid "$SESSION_ID" --arg cwd "$CWD" '{
  session_id: $sid,
  transcript_path: "/workspace/.claude/transcript-synthetic.jsonl",
  cwd: $cwd,
  hook_event_name: "PostCompact",
  trigger: "manual",
  compact_summary: "Dummy compacted summary of the synthetic conversation."
}')"

echo "[inject_synthetic_events] SubagentStart"
emit "$(jq -nc --arg sid "$SESSION_ID" --arg cwd "$CWD" '{
  session_id: $sid,
  transcript_path: "/workspace/.claude/transcript-synthetic.jsonl",
  cwd: $cwd,
  hook_event_name: "SubagentStart",
  agent_id: "agent-synthetic-0001",
  agent_type: "general-purpose"
}')"

echo "[inject_synthetic_events] SubagentStop"
emit "$(jq -nc --arg sid "$SESSION_ID" --arg cwd "$CWD" '{
  session_id: $sid,
  transcript_path: "/workspace/.claude/transcript-synthetic.jsonl",
  cwd: $cwd,
  hook_event_name: "SubagentStop",
  stop_hook_active: false,
  agent_id: "agent-synthetic-0001",
  agent_transcript_path: "/workspace/.claude/agent-transcript-synthetic.jsonl",
  agent_type: "general-purpose",
  last_assistant_message: "Dummy subagent finished its work."
}')"

echo "[inject_synthetic_events] done."
