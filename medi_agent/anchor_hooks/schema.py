"""Helpers shared by record builders."""
from __future__ import annotations

from typing import Optional


def truncate(text: Optional[str], max_len: int = 500) -> Optional[str]:
    if text is None:
        return None
    if len(text) <= max_len:
        return text
    return text[:max_len] + f"...({len(text) - max_len} more chars truncated)"
