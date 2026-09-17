"""Small local secret scanner used by the example's push path."""

from __future__ import annotations

import re

PATTERNS = {
    "OpenAI API Key": re.compile(r"sk-(proj-)?[A-Za-z0-9_-]{20,}"),
    "AWS Access Key ID": re.compile(r"AKIA[0-9A-Z]{16}"),
    "AWS Secret Access Key": re.compile(
        r"(?i)aws_secret_access_key\s*=\s*['\"][A-Za-z0-9/+=]{40}['\"]"
    ),
    "GitHub Token": re.compile(r"gh[pousr]_[A-Za-z0-9]{36,}"),
    "Slack Token": re.compile(r"xox[baprs]-[A-Za-z0-9-]{10,}"),
    "Generic Private Key Block": re.compile(r"-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    "Generic Secret Assignment": re.compile(
        r"(?i)(api[_-]?key|secret|password|token)\s*[:=]\s*['\"][A-Za-z0-9\-_/+=]{12,}['\"]"
    ),
}


def _redact(value: str) -> str:
    if len(value) <= 8:
        return "<redacted>"
    return f"{value[:4]}...{value[-4:]}"


def scan_text_for_secrets(text: str) -> list[dict[str, str | int]]:
    """Return redacted findings without copying complete credentials into logs."""

    findings: list[dict[str, str | int]] = []
    for name, pattern in PATTERNS.items():
        for match in pattern.finditer(text):
            findings.append(
                {
                    "type": name,
                    "redacted": _redact(match.group(0)),
                    "position": match.start(),
                }
            )
    return findings


def added_lines(diff: str) -> str:
    """Extract added content from a unified diff, excluding file header markers."""

    return "\n".join(
        line[1:]
        for line in diff.splitlines()
        if line.startswith("+") and not line.startswith("+++")
    )


def scan_diff_for_secrets(diff: str) -> list[dict[str, str | int]]:
    """Scan only added lines so removed or contextual secrets do not trigger alerts."""

    return scan_text_for_secrets(added_lines(diff))
