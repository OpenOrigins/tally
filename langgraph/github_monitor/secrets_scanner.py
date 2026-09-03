import re

PATTERNS = {
    "OpenAI API Key": re.compile(r"sk-(proj-)?[A-Za-z0-9]{20,}"),
    "AWS Access Key ID": re.compile(r"AKIA[0-9A-Z]{16}"),
    "AWS Secret Access Key": re.compile(r"(?i)aws_secret_access_key\s*=\s*['\"][A-Za-z0-9/+=]{40}['\"]"),
    "GitHub Token": re.compile(r"gh[pousr]_[A-Za-z0-9]{36,}"),
    "Slack Token": re.compile(r"xox[baprs]-[A-Za-z0-9-]{10,}"),
    "Generic Private Key Block": re.compile(r"-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    "Generic Secret Assignment": re.compile(
        r"(?i)(api[_-]?key|secret|password|token)\s*[:=]\s*['\"][A-Za-z0-9\-_/+=]{12,}['\"]"
    ),
}


def scan_text_for_secrets(text: str) -> list:
    """Regex-only, local, no LLM involved -- fast enough to run on every push."""
    findings = []
    for name, pattern in PATTERNS.items():
        for match in pattern.finditer(text):
            snippet = match.group(0)
            if len(snippet) > 60:
                snippet = snippet[:57] + "..."
            findings.append({"type": name, "match": snippet, "position": match.start()})
    return findings
