"""Shared TLS trust configuration for outbound HTTPS requests.

Python builds that do not link to the platform certificate store (notably the
python.org installer on macOS) cannot validate any HTTPS certificate until the
process is given an explicit CA bundle. Bundling ``certifi`` and always trusting
its bundle -- the same approach ``requests``/``httpx`` take -- means delivery and
the onboarding handshake work regardless of how the host Python was installed.
"""

from __future__ import annotations

import ssl

import certifi


def ssl_context() -> ssl.SSLContext:
    return ssl.create_default_context(cafile=certifi.where())
