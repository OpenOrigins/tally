#!/usr/bin/env python3
"""Generate random Tally agent-log records and POST them to OpenOrigins.

Examples:
  ./scripts/push_test_logs.py --env dev2 --api-key "$OO_API_KEY" --count 12
  ./scripts/push_test_logs.py --env dev2 --api-key "$OO_API_KEY" --count 5 --dry-run
  ./scripts/push_test_logs.py --url https://api.dev2.openorigins.com/v1/tally/logs \\
      --api-key "$OO_API_KEY" --count 20 --include-invalid
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import random
import string
import sys
import uuid
from datetime import datetime, timedelta, timezone
from typing import Any
from urllib.parse import urlparse

ENV_URLS = {
    "dev1": "https://api.dev1.openorigins.com/v1/tally/logs",
    "dev2": "https://api.dev2.openorigins.com/v1/tally/logs",
    "dev3": "https://api.dev3.openorigins.com/v1/tally/logs",
    "prod": "https://api.prod.openorigins.com/v1/tally/logs",
}

RECORD_TYPES = [
    "SESSION_START",
    "INSTRUCTION_RECEIVED",
    "ACTION_TAKEN",
    "RESULT_RECEIVED",
    "HANDOFF",
    "TURN_END",
    "SESSION_END",
    "HEARTBEAT",
]

ACTION_TYPES = ["read", "write", "tool_call", "decision", "handoff"]

AGENTS = [
    ("codex-container", "codex-cli"),
    ("atlas-lite", "0.3.1"),
    ("meridian-intake", "2.4.1"),
    ("castellan-editorial", "1.8.0"),
    ("demo-agent", "0.1.0"),
    ("research-bot", "nightly"),
]

TOOLS = ["Bash", "Read", "Write", "WebSearch", "ApplyPatch", "Grep"]

SOURCES = ["sdk-smoke", "push-test-logs", "load-gen", "manual-qa"]


def _rand_hex(n: int = 16) -> str:
    return "".join(random.choices("0123456789abcdef", k=n))


def _sha256(payload: str) -> str:
    return "sha256:" + hashlib.sha256(payload.encode()).hexdigest()


def _iso(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"


def _id(prefix: str) -> str:
    return f"{prefix}_{_rand_hex(16)}"


def _principal() -> dict[str, str]:
    kind = random.choice(["human", "organisation", "service"])
    if kind == "human":
        user = "".join(random.choices(string.ascii_lowercase, k=6))
        return {"type": "human", "id": f"[ARB] user:{user}@example.com"}
    if kind == "organisation":
        org = random.choice(["meridian-wire", "castellan", "openorigins-demo", "acme"])
        return {"type": "organisation", "id": f"[ARB] org:{org}"}
    return {"type": "service", "id": f"[ARB] svc:{_rand_hex(8)}"}


def build_record(
    record_type: str,
    *,
    agent_id: str,
    agent_version: str,
    session_id: str,
    run_id: str,
    ts: datetime,
    instruction_id: str | None,
    action_id: str | None,
    invalid: bool,
) -> dict[str, Any]:
    schema = "0.2"
    receipt = _id("rcpt")
    base: dict[str, Any] = {
        "record_type": record_type,
        "schema_version": schema,
        "record_id": _id("rec"),
        "run_id": run_id,
        "session_id": session_id,
        "audit_event_id": _id("evt"),
    }

    if record_type == "SESSION_START":
        base.update(
            {
                "agent_id": agent_id,
                "agent_version": agent_version,
                "principal": _principal(),
                "authority_scope_hash": _sha256(session_id + "scope"),
                "authority_scope_uri": f"private://{run_id}/authority/scope.json",
                "authority_granted_at": _iso(ts - timedelta(minutes=5)),
                "session_started_at": _iso(ts),
                "anchor_receipt": receipt,
            }
        )
    elif record_type == "INSTRUCTION_RECEIVED":
        iid = instruction_id or _id("instr")
        base.update(
            {
                "instruction_id": iid,
                "sender": {
                    "id": _principal()["id"],
                    "signature": f"sig:{_rand_hex(64)}",
                },
                "instruction_hash": _sha256(iid),
                "instruction_uri": f"private://{run_id}/{session_id}/{iid}.json",
                "instruction_received_at": _iso(ts),
                "anchor_receipt": receipt,
                "declared_intent": {
                    "summary": f"[ARB] {random.choice(['verify', 'summarise', 'edit', 'search'])} task {_rand_hex(4)}",
                    "detail_hash": _sha256("intent"),
                },
            }
        )
    elif record_type == "ACTION_TAKEN":
        aid = action_id or _id("act")
        tool = random.choice(TOOLS)
        base.update(
            {
                "action_id": aid,
                "instruction_id": instruction_id or _id("instr"),
                "action_type": random.choice(ACTION_TYPES),
                "action_timestamp": _iso(ts),
                "pre_state_hash": _sha256(f"pre-{aid}"),
                "pre_state_uri": f"private://{run_id}/{aid}-pre.json",
                "deviance_flag": {
                    "deviated": random.random() < 0.15,
                    "delta_category": None,
                    "delta_hash": None,
                    "delta_uri": None,
                },
                "tool": {
                    "name": tool,
                    "server": random.choice(["codex", "atlas", "local"]),
                    "params_hash": _sha256(tool + aid),
                    "params_uri": f"private://{run_id}/{aid}-params.json",
                },
                "anchor_receipt": receipt,
            }
        )
    elif record_type == "RESULT_RECEIVED":
        aid = action_id or _id("act")
        base.update(
            {
                "action_id": aid,
                "result_hash": _sha256(f"result-{aid}"),
                "result_uri": f"private://{run_id}/{aid}-result.json",
                "result_received_at": _iso(ts),
                "anchor_receipt": receipt,
            }
        )
    elif record_type == "HANDOFF":
        counterpart = random.choice(AGENTS)[0]
        base.update(
            {
                "handoff_id": _id("handoff"),
                "emitting_party": random.choice(["sender", "receiver"]),
                "sender": {
                    "agent_id": agent_id,
                    "org_id": f"[ARB] org:{random.choice(['meridian-wire', 'castellan', 'openorigins-demo'])}",
                    "signature": f"sig:{_rand_hex(64)}",
                },
                "receiver": {
                    "agent_id": counterpart,
                    "org_id": f"[ARB] org:{random.choice(['meridian-wire', 'castellan', 'acme'])}",
                    "signature": None,
                    "acknowledged_at": None,
                },
                "counterpart_agent_id": counterpart,
                "handoff_timestamp": _iso(ts),
                "payload_hash": _sha256(f"handoff-{session_id}"),
                "payload_uri": f"private://{run_id}/{session_id}/handoff.json",
                "anchor_receipt": receipt,
            }
        )
    elif record_type == "TURN_END":
        base.update(
            {
                "turn_id": _id("turn"),
                "outcome": random.choice(["completed", "failed", "interrupted"]),
                "outcome_hash": _sha256(f"turn-outcome-{session_id}"),
                "outcome_uri": f"private://{run_id}/{session_id}/turn-outcome.json",
                "turn_ended_at": _iso(ts),
            }
        )
    elif record_type == "SESSION_END":
        base.update(
            {
                "agent_id": agent_id,
                "outcome": random.choice(["success", "cancelled", "error", "timeout"]),
                "outcome_hash": _sha256(f"outcome-{session_id}"),
                "outcome_uri": f"private://{run_id}/{session_id}/outcome.json",
                "session_ended_at": _iso(ts),
                "anchor_receipt": receipt,
            }
        )
    elif record_type == "HEARTBEAT":
        base.update(
            {
                "agent_id": agent_id,
                "anchor_instance_id": f"anchor_inst_{_rand_hex(8)}",
                "active_sessions": [session_id],
                "timestamp": _iso(ts),
                "source": "hook-heartbeat",
                "metadata": {
                    "heartbeat_kind": random.choice(["hook-event", "timer", "probe"]),
                    "hook_event": random.choice(["SessionStart", "PreToolUse", "PostToolUse"]),
                },
                "anchor_receipt": receipt,
            }
        )

    if invalid:
        # Intentionally break compliance for pipeline testing.
        mode = random.choice(["drop_required", "bad_type", "bad_schema", "bad_action"])
        if mode == "drop_required":
            for key in ("session_id", "anchor_receipt", "agent_id", "timestamp", "action_timestamp"):
                base.pop(key, None)
        elif mode == "bad_type":
            base["record_type"] = "NOT_A_REAL_TYPE"
        elif mode == "bad_schema":
            base["schema_version"] = "9.9-broken"
        elif mode == "bad_action" and record_type == "ACTION_TAKEN":
            base["action_type"] = "explode"

    return base


def generate_records(count: int, include_invalid: bool) -> list[dict[str, Any]]:
    run_id = f"run-{_rand_hex(8)}"
    agent_id, agent_version = random.choice(AGENTS)
    # Occasionally swap agents mid-batch
    session_id = str(uuid.uuid4())
    instruction_id = _id("instr")
    action_id = _id("act")
    start = datetime.now(timezone.utc) - timedelta(seconds=random.randint(30, 600))

    # Prefer a loosely coherent session arc, then fill with random types.
    arc = [
        "SESSION_START",
        "HEARTBEAT",
        "INSTRUCTION_RECEIVED",
        "ACTION_TAKEN",
        "RESULT_RECEIVED",
        "HEARTBEAT",
        "ACTION_TAKEN",
        "RESULT_RECEIVED",
        "HANDOFF",
        "TURN_END",
        "SESSION_END",
    ]
    types: list[str] = []
    while len(types) < count:
        remaining = count - len(types)
        if remaining >= len(arc) and random.random() < 0.45:
            types.extend(arc)
            # new session for next arc
            session_id = str(uuid.uuid4())
            instruction_id = _id("instr")
            action_id = _id("act")
            if random.random() < 0.35:
                agent_id, agent_version = random.choice(AGENTS)
        else:
            types.append(random.choice(RECORD_TYPES))
    types = types[:count]

    records: list[dict[str, Any]] = []
    current_session = session_id
    current_instruction = instruction_id
    current_action = action_id
    current_agent, current_version = agent_id, agent_version

    for i, rt in enumerate(types):
        ts = start + timedelta(seconds=i * random.randint(1, 8), milliseconds=random.randint(0, 900))
        if rt == "SESSION_START":
            current_session = str(uuid.uuid4())
            current_instruction = _id("instr")
            current_action = _id("act")
            if random.random() < 0.4:
                current_agent, current_version = random.choice(AGENTS)
        if rt == "INSTRUCTION_RECEIVED":
            current_instruction = _id("instr")
        if rt == "ACTION_TAKEN":
            current_action = _id("act")

        invalid = include_invalid and random.random() < 0.25
        records.append(
            build_record(
                rt,
                agent_id=current_agent,
                agent_version=current_version,
                session_id=current_session,
                run_id=run_id,
                ts=ts,
                instruction_id=current_instruction,
                action_id=current_action,
                invalid=invalid,
            )
        )
    return records


def post_logs(
    url: str,
    api_key: str,
    body: dict[str, Any],
    *,
    source: str,
    ingest_path: str,
    timeout: float,
) -> tuple[int, str]:
    # Use http.client (not urllib.request): urllib title-cases headers to
    # "X-Api-Key", and this API Gateway authorizer only accepts lowercase
    # "x-api-key" as its identity source — anything else returns 401.
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError(f"invalid url: {url}")
    data = json.dumps(body).encode("utf-8")
    headers = {
        "Content-Type": "application/json",
        "Content-Length": str(len(data)),
        "x-api-key": api_key,
        "x-oo-tally-source": source,
        "x-oo-tally-ingest-path": ingest_path,
    }
    path = parsed.path or "/"
    if parsed.query:
        path = f"{path}?{parsed.query}"
    conn_cls = http.client.HTTPSConnection if parsed.scheme == "https" else http.client.HTTPConnection
    conn = conn_cls(parsed.hostname, parsed.port, timeout=timeout)
    try:
        conn.request("POST", path, body=data, headers=headers)
        resp = conn.getresponse()
        payload = resp.read().decode("utf-8", errors="replace")
        return resp.status, payload
    finally:
        conn.close()


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Generate random Tally logs and push them to the OpenOrigins ingest API."
    )
    p.add_argument(
        "--count",
        "-n",
        type=int,
        default=5,
        help="Number of records to generate (1-50). Default: 5",
    )
    p.add_argument(
        "--env",
        choices=sorted(ENV_URLS.keys()),
        help="Target environment (sets API URL).",
    )
    p.add_argument(
        "--url",
        help="Full ingest URL override (skips --env).",
    )
    p.add_argument(
        "--api-key",
        required=True,
        help="Org API key (x-api-key).",
    )
    p.add_argument(
        "--source",
        default="push-test-logs",
        help="x-oo-tally-source header value.",
    )
    p.add_argument(
        "--ingest-path",
        default="scripts/push_test_logs.py",
        help="x-oo-tally-ingest-path header value.",
    )
    p.add_argument(
        "--include-invalid",
        action="store_true",
        help="Randomly include intentionally non-compliant records.",
    )
    p.add_argument(
        "--one-per-request",
        action="store_true",
        help="POST each record separately instead of one {records:[...]} bundle.",
    )
    p.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the payload and do not call the API.",
    )
    p.add_argument(
        "--seed",
        type=int,
        help="RNG seed for reproducible runs.",
    )
    p.add_argument(
        "--timeout",
        type=float,
        default=30.0,
        help="HTTP timeout seconds. Default: 30",
    )
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if args.count < 1 or args.count > 50:
        print("error: --count must be between 1 and 50", file=sys.stderr)
        return 2
    if not args.url and not args.env:
        print("error: provide --env or --url", file=sys.stderr)
        return 2

    url = args.url or ENV_URLS[args.env]
    if args.seed is not None:
        random.seed(args.seed)

    records = generate_records(args.count, args.include_invalid)
    print(f"generated {len(records)} records → {url}")
    type_counts: dict[str, int] = {}
    for r in records:
        rt = str(r.get("record_type", "?"))
        type_counts[rt] = type_counts.get(rt, 0) + 1
    print("types:", ", ".join(f"{k}={v}" for k, v in sorted(type_counts.items())))

    if args.dry_run:
        body = {"records": records}
        print(json.dumps(body, indent=2))
        return 0

    if args.one_per_request:
        ok = 0
        for i, record in enumerate(records, 1):
            status, text = post_logs(
                url,
                args.api_key,
                record,
                source=args.source,
                ingest_path=f"{args.ingest_path}#{i}",
                timeout=args.timeout,
            )
            print(f"[{i}/{len(records)}] HTTP {status}: {text[:300]}")
            if 200 <= status < 300:
                ok += 1
        print(f"done: {ok}/{len(records)} accepted")
        return 0 if ok == len(records) else 1

    status, text = post_logs(
        url,
        args.api_key,
        {"records": records},
        source=random.choice(SOURCES) if args.source == "push-test-logs" else args.source,
        ingest_path=args.ingest_path,
        timeout=args.timeout,
    )
    print(f"HTTP {status}")
    try:
        print(json.dumps(json.loads(text), indent=2))
    except json.JSONDecodeError:
        print(text)
    return 0 if 200 <= status < 300 else 1


if __name__ == "__main__":
    raise SystemExit(main())
