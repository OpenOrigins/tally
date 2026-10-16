# Task metadata from the active turn

Tally obtains task matching and task descriptions from the Codex or Claude Code
turn the user is already running. It does not start either client's CLI. If the
client does not provide task metadata, the backend may generate a task summary
after the delivery grace period.

## Protocol

1. The installed `UserPromptSubmit` command hook records the prompt, requests
   active task candidates from the authenticated Tally ingest API, and returns
   `hookSpecificOutput.additionalContext`. It offers up to 24 candidates,
   ranked by overlap with the prompt, with the session's linked task first.
2. The assistant works normally. The injected context asks it to end its
   answer with one `<!-- tally-task-assist-v1:{...} -->` comment containing
   `decision`, `existing_task_id`, `title`, `goal`, and `progress`.
3. The `Stop` hook reads `last_assistant_message`, validates the comment,
   checks that a selected task was offered, and hands an authenticated
   `submit` request to Tally's own delivery helper. The helper retries while
   ingest materializes the session. The backend rechecks ownership, source,
   task state, timing, and duplicate turns before storing anything. Failed
   delivery packets remain on disk and are retried by the next client hook.
4. Missing hooks, unavailable candidates, malformed model output, and
   delivery failures leave the user turn alone. The backend's deterministic
   matcher and summary fallback run after the turn ends and the client grace period.

The HTML comment is hidden in most rendered Markdown views but can appear in
raw transcripts. For explicit exact-format prompts Tally skips the comment and
uses the backend fallback. The hook context also tells the assistant to omit it
when an output schema requires a strict response. A CLI output schema is not
currently included in the prompt-hook payload, so automatic detection depends
on the assistant following that instruction. The comment is necessary because
the portable Stop-hook interface exposes the final assistant message, not a
private model result channel.
Tally does not use a blocking Stop-hook retry: a retry would create an extra
user-visible assistant continuation and could overwrite the prompt snapshot.

This first version keeps a session's client-selected task link. A late client
decision can replace a provisional backend match. It does not split a
single Codex or Claude Code session into separate tasks when the user changes
goals midway through that session. The backend places new turns on the linked
task without a second model-based match for those clients.

## Installation and compatibility

The existing Tally installer writes the Tally helper binary, credentials,
Codex `config.toml` hooks and notify callback, or Claude Code `settings.json`
hooks. The Codex installer still checks for a Codex CLI with lifecycle hook
support and asks the user to review and trust the hooks there. The task
metadata algorithm itself does not invoke the Codex or Claude CLI. It does
not edit global `AGENTS.md` or `CLAUDE.md`, install a skill, or require MCP.

Codex command hooks still require that the user trust their exact definition.
Tally does not write trust hashes or bypass that review. If hooks have not been
trusted or are disabled, task metadata uses the deterministic backend fallback.
Claude Code command hooks use its ordinary settings mechanism. Disabling MCP
does not affect this protocol; disabling command hooks does.

No production configuration is changed by source builds or tests. The
`TALLY_TASK_ASSIST_ENABLED=0` environment setting disables the in-turn
metadata path.
